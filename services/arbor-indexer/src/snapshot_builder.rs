//! Snapshot builder: converts a [`Graph`] into a read-optimized [`Snapshot`].
//!
//! See [`SnapshotBuilder::build`] for the algorithm overview.

use arbor_bytecode::BytecodeCompiler;
use arbor_graph_core::{graph::Graph, types::NodeType};
use arbor_index_snapshot::Snapshot;
use arbor_types::{
    Action, ActionSet, ArborError, ArborResult, CompiledCondition, Entity,
    EntityTypeId, IndexedEntity, IndexedEntityType, IndexedNode, IndexedPolicy,
    IndexedPolicyTarget, Policy, PolicyTarget, PolicyType,
};
use rapidhash::RapidHashMap;
use roaring::RoaringBitmap;
use uuid::Uuid;

use crate::closures::{compute_all_descendants, compute_ancestors};

// ---------------------------------------------------------------------------
// Free helper functions
// ---------------------------------------------------------------------------

fn new_entity_type() -> IndexedEntityType {
    IndexedEntityType {
        nodes_of_type: RoaringBitmap::new(),
        policies_targeting_principals_of_type: RoaringBitmap::new(),
        policies_targeting_resources_of_type: RoaringBitmap::new(),
    }
}

fn map_target(
    target: &PolicyTarget,
    uuid_to_index: &RapidHashMap<Uuid, u32>,
) -> ArborResult<IndexedPolicyTarget> {
    match target {
        PolicyTarget::Entity(uuid) => uuid_to_index
            .get(uuid)
            .copied()
            .map(IndexedPolicyTarget::Entity)
            .ok_or_else(|| ArborError::EntityNotFound(uuid.to_string())),
        PolicyTarget::EntityWithDescendants(uuid) => uuid_to_index
            .get(uuid)
            .copied()
            .map(IndexedPolicyTarget::EntityWithDescendants)
            .ok_or_else(|| ArborError::EntityNotFound(uuid.to_string())),
        PolicyTarget::EntityType(type_id) => Ok(IndexedPolicyTarget::EntityType(*type_id)),
        PolicyTarget::All => Ok(IndexedPolicyTarget::All),
    }
}

fn expand_actions(
    policy: &Policy,
    uuid_to_index: &RapidHashMap<Uuid, u32>,
    nodes: &[NodeType],
) -> RoaringBitmap {
    let mut actions = RoaringBitmap::new();

    for uuid in &policy.actions {
        if let Some(&i) = uuid_to_index.get(uuid) {
            actions.insert(i);
        }
    }
    for set_uuid in &policy.action_sets {
        if let Some(&set_idx) = uuid_to_index.get(set_uuid)
            && let Some(NodeType::ActionSet(action_set)) = nodes.get(set_idx as usize) {
                for uuid in &action_set.actions {
                    if let Some(&i) = uuid_to_index.get(uuid) {
                        actions.insert(i);
                    }
                }
            }
    }

    actions
}

fn compile_condition(policy: &Policy) -> Option<CompiledCondition> {
    let condition = policy.conditions.as_ref()?;
    match BytecodeCompiler::new().compile(condition) {
        Ok(cc) => Some(cc),
        Err(err) => {
            eprintln!(
                "[arbor-indexer] WARNING: failed to compile condition for \
                 policy {} ({}): {err}; condition treated as absent",
                policy.name, policy.id
            );
            None
        }
    }
}

// ---------------------------------------------------------------------------
// BuildState — accumulates all snapshot indexes during the single node scan.
// ---------------------------------------------------------------------------

struct BuildState {
    nodes: Vec<IndexedNode>,
    indexed_entity_types: RapidHashMap<EntityTypeId, IndexedEntityType>,
    entity_type_name_to_id: RapidHashMap<String, EntityTypeId>,
    action_to_policies: RapidHashMap<u32, RoaringBitmap>,
    index_to_uuid: Vec<Option<Uuid>>,

    all_principal_policies: RoaringBitmap,
    all_resource_policies: RoaringBitmap,
    conditional_policies: RoaringBitmap,
    forbidding_policies: RoaringBitmap,
    descendant_principal_policies: RoaringBitmap,
    descendant_resource_policies: RoaringBitmap,

    /// Transitive descendants per entity index, computed during `process_entity`.
    /// Index-time only; not stored on `IndexedEntity`.
    entity_descendants: Vec<RoaringBitmap>,

    /// Deferred (entity_idx, policy_idx) pairs written back after the full
    /// node scan, once all entities are in place.
    deferred_principal: Vec<(u32, u32)>,
    deferred_resource: Vec<(u32, u32)>,
}

impl BuildState {
    fn new(node_count: usize, entity_descendants: Vec<RoaringBitmap>) -> Self {
        Self {
            nodes: (0..node_count).map(|_| IndexedNode::Other).collect(),
            indexed_entity_types: RapidHashMap::default(),
            entity_type_name_to_id: RapidHashMap::default(),
            action_to_policies: RapidHashMap::default(),
            index_to_uuid: vec![None; node_count],
            all_principal_policies: RoaringBitmap::new(),
            all_resource_policies: RoaringBitmap::new(),
            conditional_policies: RoaringBitmap::new(),
            forbidding_policies: RoaringBitmap::new(),
            descendant_principal_policies: RoaringBitmap::new(),
            descendant_resource_policies: RoaringBitmap::new(),
            entity_descendants,
            deferred_principal: Vec::new(),
            deferred_resource: Vec::new(),
        }
    }

    fn process_entity(&mut self, idx: u32, entity: &Entity, graph: &Graph) {
        self.index_to_uuid[idx as usize] = Some(entity.id);

        let mut ancestors = compute_ancestors(&graph.parents, idx);
        ancestors.insert(idx); // self-inclusive (InHierarchy invariant)
        // entity_descendants[idx] already populated by compute_all_descendants

        self.indexed_entity_types
            .entry(entity.entity_type)
            .or_insert_with(new_entity_type)
            .nodes_of_type
            .insert(idx);

        self.nodes[idx as usize] = IndexedNode::Entity(IndexedEntity {
            idx,
            attributes: entity.attributes.clone(),
            entity_type: entity.entity_type,
            ancestors,
            principal_of_policies: None,
            resource_of_policies: None,
            effective_principal_policies: None,
            effective_resource_policies: None,
        });
    }

    fn process_policy(
        &mut self,
        idx: u32,
        policy: &Policy,
        graph: &Graph,
    ) -> ArborResult<()> {
        self.index_to_uuid[idx as usize] = Some(policy.id);

        let principal_target = map_target(&policy.principal, &graph.uuid_to_index)?;
        let resource_target  = map_target(&policy.resource,  &graph.uuid_to_index)?;
        let actions          = expand_actions(policy, &graph.uuid_to_index, &graph.nodes);
        let is_conditional   = policy.conditions.is_some();
        let conditions       = compile_condition(policy);
        let is_forbidding    = policy.policy_type == PolicyType::Forbid;

        if is_conditional { self.conditional_policies.insert(idx); }
        if is_forbidding  { self.forbidding_policies.insert(idx); }

        self.classify_principal(idx, &principal_target);
        self.classify_resource(idx, &resource_target);

        for action_idx in &actions {
            self.action_to_policies
                .entry(action_idx)
                .or_default()
                .insert(idx);
        }

        self.nodes[idx as usize] = IndexedNode::Policy(IndexedPolicy {
            idx,
            principal_target,
            resource_target,
            actions,
            conditions,
            is_forbidding,
            is_conditional,
            principal_descendants: None,
            resource_descendants: None,
        });

        Ok(())
    }

    fn process_action(&mut self, idx: u32, action: &Action) {
        self.index_to_uuid[idx as usize] = Some(action.id);
    }

    fn process_action_set(&mut self, idx: u32, action_set: &ActionSet) {
        self.index_to_uuid[idx as usize] = Some(action_set.id);
    }

    fn classify_principal(&mut self, policy_idx: u32, target: &IndexedPolicyTarget) {
        match target {
            IndexedPolicyTarget::All => {
                self.all_principal_policies.insert(policy_idx);
            }
            IndexedPolicyTarget::EntityWithDescendants(entity_idx) => {
                self.descendant_principal_policies.insert(policy_idx);
                self.deferred_principal.push((*entity_idx, policy_idx));
            }
            IndexedPolicyTarget::Entity(entity_idx) => {
                self.deferred_principal.push((*entity_idx, policy_idx));
            }
            IndexedPolicyTarget::EntityType(type_id) => {
                self.indexed_entity_types
                    .entry(*type_id)
                    .or_insert_with(new_entity_type)
                    .policies_targeting_principals_of_type
                    .insert(policy_idx);
            }
        }
    }

    fn classify_resource(&mut self, policy_idx: u32, target: &IndexedPolicyTarget) {
        match target {
            IndexedPolicyTarget::All => {
                self.all_resource_policies.insert(policy_idx);
            }
            IndexedPolicyTarget::EntityWithDescendants(entity_idx) => {
                self.descendant_resource_policies.insert(policy_idx);
                self.deferred_resource.push((*entity_idx, policy_idx));
            }
            IndexedPolicyTarget::Entity(entity_idx) => {
                self.deferred_resource.push((*entity_idx, policy_idx));
            }
            IndexedPolicyTarget::EntityType(type_id) => {
                self.indexed_entity_types
                    .entry(*type_id)
                    .or_insert_with(new_entity_type)
                    .policies_targeting_resources_of_type
                    .insert(policy_idx);
            }
        }
    }

    fn apply_deferred(&mut self) {
        for (entity_idx, policy_idx) in self.deferred_principal.drain(..) {
            if let Some(IndexedNode::Entity(e)) = self.nodes.get_mut(entity_idx as usize) {
                e.principal_of_policies
                    .get_or_insert_with(RoaringBitmap::new)
                    .insert(policy_idx);
            }
        }
        for (entity_idx, policy_idx) in self.deferred_resource.drain(..) {
            if let Some(IndexedNode::Entity(e)) = self.nodes.get_mut(entity_idx as usize) {
                e.resource_of_policies
                    .get_or_insert_with(RoaringBitmap::new)
                    .insert(policy_idx);
            }
        }
    }

    /// Pass 1: stamp `principal_descendants` / `resource_descendants` onto each
    /// `EntityWithDescendants` policy using the pre-computed `entity_descendants` table.
    ///
    /// Pass 2: for every entity compute and store its effective policy union as
    /// `effective_principal_policies` / `effective_resource_policies`.
    fn compute_derived_fields(&mut self) {
        // Single scan: collect all derived values into temp arrays, then write back.
        //
        // For each policy node: copy its (Copy) targets, release the borrow on `self.nodes`,
        // then read `self.entity_descendants` to build the descendant bitmaps.
        // For each entity node: clone the fields needed for the ancestor walk, release the
        // borrow, then read `self.nodes[anc_idx]` freely.
        let node_count = self.nodes.len();
        let mut policy_principal_desc: Vec<Option<RoaringBitmap>> = vec![None; node_count];
        let mut policy_resource_desc:  Vec<Option<RoaringBitmap>> = vec![None; node_count];
        let mut effective_principal:   Vec<Option<RoaringBitmap>> = vec![None; node_count];
        let mut effective_resource:    Vec<Option<RoaringBitmap>> = vec![None; node_count];

        for idx in 0..node_count {
            // --- Policy: copy targets (Copy type) and release borrow before cross-field reads ---
            let policy_targets = match &self.nodes[idx] {
                IndexedNode::Policy(p) => Some((p.principal_target, p.resource_target)),
                _ => None,
            };
            if let Some((pt, rt)) = policy_targets {
                if let IndexedPolicyTarget::EntityWithDescendants(tidx) = pt {
                    policy_principal_desc[idx] = Some(self.entity_descendants[tidx as usize].clone());
                }
                if let IndexedPolicyTarget::EntityWithDescendants(tidx) = rt {
                    policy_resource_desc[idx] = Some(self.entity_descendants[tidx as usize].clone());
                }
                continue;
            }

            // --- Entity: clone needed fields and release borrow before ancestor walk ---
            let entity_data = match &self.nodes[idx] {
                IndexedNode::Entity(e) => Some((
                    e.ancestors.clone(),
                    e.entity_type,
                    e.principal_of_policies.clone(),
                    e.resource_of_policies.clone(),
                )),
                _ => None,
            };
            let Some((ancestors, entity_type, principal_of, resource_of)) = entity_data else { continue };

            let mut acc_p = self.all_principal_policies.clone();
            if let Some(ref direct) = principal_of { acc_p |= direct; }
            let mut acc_r = self.all_resource_policies.clone();
            if let Some(ref direct) = resource_of { acc_r |= direct; }

            for anc_idx in ancestors.iter() {
                if let Some(IndexedNode::Entity(anc)) = self.nodes.get(anc_idx as usize) {
                    if let Some(p) = &anc.principal_of_policies {
                        acc_p |= p & &self.descendant_principal_policies;
                    }
                    if let Some(r) = &anc.resource_of_policies {
                        acc_r |= r & &self.descendant_resource_policies;
                    }
                }
            }

            if let Some(et) = self.indexed_entity_types.get(&entity_type) {
                acc_p |= &et.policies_targeting_principals_of_type;
                acc_r |= &et.policies_targeting_resources_of_type;
            }

            effective_principal[idx] = Some(acc_p);
            effective_resource[idx]  = Some(acc_r);
        }

        // Write back.
        for (idx, node) in self.nodes.iter_mut().enumerate() {
            match node {
                IndexedNode::Policy(p) => {
                    p.principal_descendants = policy_principal_desc[idx].take();
                    p.resource_descendants  = policy_resource_desc[idx].take();
                }
                IndexedNode::Entity(e) => {
                    e.effective_principal_policies = effective_principal[idx].take();
                    e.effective_resource_policies  = effective_resource[idx].take();
                }
                _ => {}
            }
        }
    }

    fn into_snapshot(self, uuid_to_index: RapidHashMap<Uuid, u32>) -> Snapshot {
        Snapshot {
            uuid_to_index,
            index_to_uuid: self.index_to_uuid,
            nodes: self.nodes,
            action_to_policies: self.action_to_policies,
            indexed_entity_types: self.indexed_entity_types,
            entity_type_name_to_id: self.entity_type_name_to_id,
            all_principal_policies: self.all_principal_policies,
            all_resource_policies: self.all_resource_policies,
            conditional_policies: self.conditional_policies,
            forbidding_policies: self.forbidding_policies,
            descendant_principal_policies: self.descendant_principal_policies,
            descendant_resource_policies: self.descendant_resource_policies,
        }
    }
}

// ---------------------------------------------------------------------------
// SnapshotBuilder
// ---------------------------------------------------------------------------

pub struct SnapshotBuilder;

impl SnapshotBuilder {
    /// Build a read-optimized [`Snapshot`] from the supplied graph.
    ///
    /// Single forward scan over `graph.nodes`, followed by two deferred
    /// write-backs for `principal_of_policies` / `resource_of_policies`.
    ///
    /// # Errors
    ///
    /// Returns [`ArborError::EntityNotFound`] if a policy references a
    /// principal or resource UUID not present in the graph.
    pub fn build(graph: &Graph) -> ArborResult<Snapshot> {
        let entity_descendants = compute_all_descendants(&graph.children, graph.nodes.len());
        let mut state = BuildState::new(graph.nodes.len(), entity_descendants);

        // Copy entity type names
        for (id, name) in &graph.entity_type_names {
            state.entity_type_name_to_id.insert(name.clone(), *id);
        }

        for (idx, node) in graph.nodes.iter().enumerate() {
            let idx = idx as u32;
            match node {
                NodeType::Entity(entity)        => state.process_entity(idx, entity, graph),
                NodeType::Policy(policy)        => state.process_policy(idx, policy, graph)?,
                NodeType::Action(action)        => state.process_action(idx, action),
                NodeType::ActionSet(action_set) => state.process_action_set(idx, action_set),
                NodeType::Placeholder           => {}
            }
        }

        state.apply_deferred();
        state.compute_derived_fields();
        Ok(state.into_snapshot(graph.uuid_to_index.clone()))
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use arbor_graph_core::graph::Graph;
    use arbor_types::EntityTypeId;

    #[test]
    fn test_entity_type_name_mapping() {
        let mut graph = Graph::new();
        let type_id = EntityTypeId::new(1);
        graph.register_entity_type(type_id, "User".to_string());

        let snapshot = SnapshotBuilder::build(&graph).expect("build failed");

        assert_eq!(snapshot.get_entity_type_id_by_name("User"), Some(type_id));
        assert_eq!(snapshot.get_entity_type_id_by_name("NonExistent"), None);
    }
}
