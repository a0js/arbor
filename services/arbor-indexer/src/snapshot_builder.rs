//! Snapshot builder: converts a [`Graph`] into a read-optimized [`Snapshot`].
//!
//! See [`SnapshotBuilder::build`] for the algorithm overview.

use arbor_bytecode::BytecodeCompiler;
use arbor_graph_core::{graph::Graph, types::NodeType};
use arbor_index_snapshot::Snapshot;
use arbor_types::{
    Action, ActionSet, ArborError, ArborResult, CompiledCondition, Entity, EntityTypeId,
    IndexedAttributeValue, IndexedEntity, IndexedEntityType, IndexedNode, IndexedPolicy,
    IndexedPolicyTarget, Policy, PolicyTarget, PolicyType, SortedSetRef, AttributeNameId,
    flatten_attributes,
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

    /// Shared arena backing every entity's `ancestors: SortedSetRef`, appended
    /// to in `process_entity` as nodes are scanned in index order.
    ancestors_arena: Vec<u32>,
    /// Shared arenas for the other per-entity `SortedSetRef` fields, filled
    /// in during `apply_deferred` / `compute_derived_fields`.
    principal_of_arena: Vec<u32>,
    resource_of_arena: Vec<u32>,
    effective_principal_arena: Vec<u32>,
    effective_resource_arena: Vec<u32>,
    /// Backs every entity's `attributes: SortedSetRef`, filled in
    /// `process_entity` by flattening the graph-level `Attributes`
    /// (`BTreeMap`) into named pairs / unnamed Set elements.
    attribute_pairs_arena: Vec<(AttributeNameId, IndexedAttributeValue)>,
    attribute_set_values_arena: Vec<IndexedAttributeValue>,
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
    /// Descendant sets actually referenced by an `EntityWithDescendants`
    /// policy target, keyed by target entity index and deduplicated --
    /// multiple policies targeting the same root share one entry instead of
    /// each cloning `entity_descendants[tidx]` into its own field. Replaces
    /// the old per-policy `principal_descendants`/`resource_descendants`.
    descendants_by_target: RapidHashMap<u32, RoaringBitmap>,

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
            ancestors_arena: Vec::new(),
            principal_of_arena: Vec::new(),
            resource_of_arena: Vec::new(),
            effective_principal_arena: Vec::new(),
            effective_resource_arena: Vec::new(),
            attribute_pairs_arena: Vec::new(),
            attribute_set_values_arena: Vec::new(),
            action_to_policies: RapidHashMap::default(),
            index_to_uuid: vec![None; node_count],
            all_principal_policies: RoaringBitmap::new(),
            all_resource_policies: RoaringBitmap::new(),
            conditional_policies: RoaringBitmap::new(),
            forbidding_policies: RoaringBitmap::new(),
            descendant_principal_policies: RoaringBitmap::new(),
            descendant_resource_policies: RoaringBitmap::new(),
            entity_descendants,
            descendants_by_target: RapidHashMap::default(),
            deferred_principal: Vec::new(),
            deferred_resource: Vec::new(),
        }
    }

    fn process_entity(&mut self, idx: u32, entity: &Entity, graph: &Graph) {
        self.index_to_uuid[idx as usize] = Some(entity.id);

        let mut ancestors_bitmap = compute_ancestors(&graph.parents, idx);
        ancestors_bitmap.insert(idx); // self-inclusive (InHierarchy invariant)
        // entity_descendants[idx] already populated by compute_all_descendants

        // RoaringBitmap iterates in ascending order, so this keeps the arena
        // slice sorted (required for binary_search at query time).
        let offset = self.ancestors_arena.len() as u32;
        self.ancestors_arena.extend(ancestors_bitmap.iter());
        let ancestors = SortedSetRef { offset, len: ancestors_bitmap.len() as u32 };

        self.indexed_entity_types
            .entry(entity.entity_type)
            .or_insert_with(new_entity_type)
            .nodes_of_type
            .insert(idx);

        let attributes = flatten_attributes(
            &entity.attributes,
            &mut self.attribute_pairs_arena,
            &mut self.attribute_set_values_arena,
        );

        self.nodes[idx as usize] = IndexedNode::Entity(IndexedEntity {
            idx,
            attributes,
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

        self.nodes[idx as usize] = IndexedNode::Policy(Box::new(IndexedPolicy {
            idx,
            principal_target,
            resource_target,
            conditions,
            is_forbidding,
            is_conditional,
        }));

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
        // Group by entity first (an entity can be the direct target of
        // multiple policies), then write each entity's sorted, deduplicated
        // list into the shared arena as one contiguous range -- appending
        // incrementally per (entity, policy) pair would scatter one entity's
        // policies across non-contiguous arena regions.
        let mut principal_of: RapidHashMap<u32, Vec<u32>> = RapidHashMap::default();
        for (entity_idx, policy_idx) in self.deferred_principal.drain(..) {
            principal_of.entry(entity_idx).or_default().push(policy_idx);
        }
        let mut resource_of: RapidHashMap<u32, Vec<u32>> = RapidHashMap::default();
        for (entity_idx, policy_idx) in self.deferred_resource.drain(..) {
            resource_of.entry(entity_idx).or_default().push(policy_idx);
        }

        // Sort by entity index for a deterministic arena layout.
        let mut principal_entries: Vec<(u32, Vec<u32>)> = principal_of.into_iter().collect();
        principal_entries.sort_unstable_by_key(|(idx, _)| *idx);
        for (entity_idx, mut policies) in principal_entries {
            policies.sort_unstable();
            policies.dedup();
            let offset = self.principal_of_arena.len() as u32;
            let len = policies.len() as u32;
            self.principal_of_arena.extend_from_slice(&policies);
            if let Some(IndexedNode::Entity(e)) = self.nodes.get_mut(entity_idx as usize) {
                e.principal_of_policies = Some(SortedSetRef { offset, len });
            }
        }

        let mut resource_entries: Vec<(u32, Vec<u32>)> = resource_of.into_iter().collect();
        resource_entries.sort_unstable_by_key(|(idx, _)| *idx);
        for (entity_idx, mut policies) in resource_entries {
            policies.sort_unstable();
            policies.dedup();
            let offset = self.resource_of_arena.len() as u32;
            let len = policies.len() as u32;
            self.resource_of_arena.extend_from_slice(&policies);
            if let Some(IndexedNode::Entity(e)) = self.nodes.get_mut(entity_idx as usize) {
                e.resource_of_policies = Some(SortedSetRef { offset, len });
            }
        }
    }

    /// Pass 1: populate `descendants_by_target` with one deduplicated entry
    /// per distinct `EntityWithDescendants` target, using the pre-computed
    /// `entity_descendants` table -- policies sharing a target share the
    /// entry instead of each cloning their own copy.
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
        // Sorted element lists (not RoaringBitmap) since these become
        // arena-backed SortedSetRefs, not per-entity bitmaps.
        let mut effective_principal:   Vec<Option<Vec<u32>>> = vec![None; node_count];
        let mut effective_resource:    Vec<Option<Vec<u32>>> = vec![None; node_count];

        for idx in 0..node_count {
            // --- Policy: copy targets (Copy type) and release borrow before cross-field reads ---
            let policy_targets = match &self.nodes[idx] {
                IndexedNode::Policy(p) => Some((p.principal_target, p.resource_target)),
                _ => None,
            };
            if let Some((pt, rt)) = policy_targets {
                if let IndexedPolicyTarget::EntityWithDescendants(tidx) = pt {
                    self.descendants_by_target
                        .entry(tidx)
                        .or_insert_with(|| self.entity_descendants[tidx as usize].clone());
                }
                if let IndexedPolicyTarget::EntityWithDescendants(tidx) = rt {
                    self.descendants_by_target
                        .entry(tidx)
                        .or_insert_with(|| self.entity_descendants[tidx as usize].clone());
                }
                continue;
            }

            // --- Entity: copy needed fields (all Copy) and release borrow before ancestor walk ---
            let entity_data = match &self.nodes[idx] {
                IndexedNode::Entity(e) => Some((
                    e.ancestors,
                    e.entity_type,
                    e.principal_of_policies,
                    e.resource_of_policies,
                )),
                _ => None,
            };
            let Some((ancestors, entity_type, principal_of, resource_of)) = entity_data else { continue };

            let mut acc_p = self.all_principal_policies.clone();
            if let Some(r) = principal_of {
                let slice = &self.principal_of_arena[r.offset as usize..(r.offset + r.len) as usize];
                acc_p.extend(slice.iter().copied());
            }
            let mut acc_r = self.all_resource_policies.clone();
            if let Some(r) = resource_of {
                let slice = &self.resource_of_arena[r.offset as usize..(r.offset + r.len) as usize];
                acc_r.extend(slice.iter().copied());
            }

            let ancestors_slice = &self.ancestors_arena
                [ancestors.offset as usize..(ancestors.offset + ancestors.len) as usize];
            for &anc_idx in ancestors_slice {
                if let Some(IndexedNode::Entity(anc)) = self.nodes.get(anc_idx as usize) {
                    if let Some(p_ref) = anc.principal_of_policies {
                        let p_slice = &self.principal_of_arena
                            [p_ref.offset as usize..(p_ref.offset + p_ref.len) as usize];
                        for &pol in p_slice {
                            if self.descendant_principal_policies.contains(pol) {
                                acc_p.insert(pol);
                            }
                        }
                    }
                    if let Some(r_ref) = anc.resource_of_policies {
                        let r_slice = &self.resource_of_arena
                            [r_ref.offset as usize..(r_ref.offset + r_ref.len) as usize];
                        for &pol in r_slice {
                            if self.descendant_resource_policies.contains(pol) {
                                acc_r.insert(pol);
                            }
                        }
                    }
                }
            }

            if let Some(et) = self.indexed_entity_types.get(&entity_type) {
                acc_p |= &et.policies_targeting_principals_of_type;
                acc_r |= &et.policies_targeting_resources_of_type;
            }

            // RoaringBitmap iterates in ascending order, so these stay sorted.
            effective_principal[idx] = Some(acc_p.iter().collect());
            effective_resource[idx]  = Some(acc_r.iter().collect());
        }

        // Write back.
        for (idx, node) in self.nodes.iter_mut().enumerate() {
            match node {
                IndexedNode::Entity(e) => {
                    if let Some(policies) = effective_principal[idx].take() {
                        let offset = self.effective_principal_arena.len() as u32;
                        let len = policies.len() as u32;
                        self.effective_principal_arena.extend(policies);
                        e.effective_principal_policies = Some(SortedSetRef { offset, len });
                    }
                    if let Some(policies) = effective_resource[idx].take() {
                        let offset = self.effective_resource_arena.len() as u32;
                        let len = policies.len() as u32;
                        self.effective_resource_arena.extend(policies);
                        e.effective_resource_policies = Some(SortedSetRef { offset, len });
                    }
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
            ancestors_arena: self.ancestors_arena,
            principal_of_arena: self.principal_of_arena,
            resource_of_arena: self.resource_of_arena,
            effective_principal_arena: self.effective_principal_arena,
            effective_resource_arena: self.effective_resource_arena,
            attribute_pairs_arena: self.attribute_pairs_arena,
            attribute_set_values_arena: self.attribute_set_values_arena,
            action_to_policies: self.action_to_policies,
            descendants_by_target: self.descendants_by_target,
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
