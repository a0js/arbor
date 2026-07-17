use uuid::Uuid;
use arbor_types::{
    Action, ActionSet, ArborError, ArborResult, AttributeNameId, AttributeValue, AttributeValueInput,
    Attributes, Condition, ConditionInput, Entity, EntityInput, EntityTypeId, GraphError, Operand,
    OperandInput, Policy, PolicyInput, PolicyTarget, PolicyTargetInput, VariableRef,
};
use roaring::RoaringBitmap;
use crate::types::NodeType;

/// Indices returned by `validate_policy_relationships`:
/// (principal_index, resource_index, action_indices, action_set_indices)
type PolicyRelIndices = (Option<u32>, Option<u32>, Vec<u32>, Vec<u32>);

enum NodeNewOrExisting {
    New(u32),
    Existing(u32),
}

impl NodeNewOrExisting {
    pub(crate) fn value(&self) -> u32 {
        match self {
            NodeNewOrExisting::New(idx) | NodeNewOrExisting::Existing(idx) => *idx,
        }
    }

    pub(crate) fn is_new(&self) -> bool {
        matches!(self, NodeNewOrExisting::New(_))
    }

    pub(crate) fn is_existing(&self) -> bool {
        matches!(self, NodeNewOrExisting::Existing(_))
    }
}

impl super::graph::Graph {
    /// Upsert an entity in the graph (insert if new, update if existing)
    /// This method handles all entity modifications including relationships and metadata
    pub fn upsert_entity(&mut self, entity: Entity) -> ArborResult<()> {
        let entity_id = entity.id;

        let index = self.get_or_create_index(&entity_id);

        let idx = index.value();
        // Check for circular dependencies before proceeding, only possible if the index already exists (i.e. a placeholder or an existing entity)
        if index.is_existing() {
            for parent in &entity.parents {
                let Some(parent_index) = self.uuid_to_index.get(parent).copied() else {
                    continue;
                };

                let mut visited = RoaringBitmap::new();
                visited.insert(idx);
                let mut current_path = vec![idx];

                // Check for circular dependencies
                if self.dfs_has_parent_cycle(parent_index, &mut visited, &mut current_path) {
                    return Err(ArborError::CircularDependency(format!(
                        "Circular dependency detected: {:?}",
                        current_path
                    )));
                }
            }
        }

        if index.is_existing() {
            self.clear_parental_relationships(idx);
        };

        for parent in &entity.parents {
            let parent_index = self.get_or_create_index(parent).value();
            self.add_parental_relationships(idx, parent_index);
        }

        match self.nodes.get_mut(idx as usize) {
            Some(element) => *element = NodeType::Entity(Box::new(entity)),
            None => return Err(ArborError::Graph(GraphError::NodeIndexNotFound(format!("Entity Index out of bounds: {}", idx)))),
        };
        Ok(())
    }

    pub fn remove_entity(&mut self, entity_id: Uuid) -> ArborResult<()> {
        let (entity_index, _entity) = self.verify_entity_existence(entity_id)?;

        self.clear_parental_relationships(entity_index);
        self.uuid_to_index.remove(&entity_id);
        let _ = self.free_index(entity_index);

        Ok(())
    }

    /// Upsert a policy in the graph (insert if new, update if existing)
    /// This method handles all policy modifications including target updates
    pub fn upsert_policy(&mut self, policy: Policy) -> ArborResult<()> {
        let policy_id = policy.id;

        let (_principal_index, _resource_index, _action_indices, _action_set_indices) =
            self.validate_policy_relationships(&policy)?;

        // Get or create index for the policy
        let policy_index = self.get_or_create_index(&policy_id);
        let policy_idx = policy_index.value();
        // Reset all relationships for the policy

        match self.nodes.get_mut(policy_idx as usize) {
            Some(element) => *element = NodeType::Policy(Box::new(policy)),
            None => return Err(ArborError::Graph(GraphError::NodeIndexNotFound(format!("Policy Index out of bounds: {}", policy_idx)))),
        };

        Ok(())
    }

    /// Remove a policy from the graph
    pub fn remove_policy(&mut self, policy_id: Uuid) -> ArborResult<()> {
        let (policy_index, _) = self.verify_policy_existence(policy_id)?;

        self.uuid_to_index.remove(&policy_id);
        let _ = self.free_index(policy_index);
        Ok(())
    }

    /// Add an action to the graph
    pub fn add_action(&mut self, action: Action) -> ArborResult<()> {
        let action_id = action.id;
        let action_index = self.get_or_create_index(&action_id);
        if action_index.is_new() {
            let action_idx = action_index.value();
            match self.nodes.get_mut(action_idx as usize) {
                Some(element) => *element = NodeType::Action(Box::new(action)),
                None => return Err(ArborError::Graph(GraphError::NodeIndexNotFound(format!("Action Index out of bounds: {}", action_idx)))),
            };
        } else {
            return Err(ArborError::Graph(GraphError::NodeAlreadyExists(
                "Action::".to_string() + &action_id.to_string(),
            )));
        }

        Ok(())
    }

    pub fn remove_action(&mut self, action_id: Uuid) -> ArborResult<()> {
        let (action_index, _) = self.verify_action_existence(action_id)?;

        self.uuid_to_index.remove(&action_id);
        let _ = self.free_index(action_index);
        Ok(())
    }

    /// Add an action set to the graph
    pub fn upsert_action_set(&mut self, action_set: ActionSet) -> ArborResult<()> {
        let action_set_id = action_set.id;

        // Get or create index for the action set
        let action_set_index = self.get_or_create_index(&action_set_id);
        let action_set_idx = action_set_index.value();

        match self.nodes.get_mut(action_set_idx as usize) {
            Some(element) => *element = NodeType::ActionSet(Box::new(action_set)),
            None => return Err(ArborError::Graph(GraphError::NodeIndexNotFound(format!("Action Set Index out of bounds: {}", action_set_idx)))),
        };

        Ok(())
    }

    pub fn remove_action_set(&mut self, action_set_id: Uuid) -> ArborResult<()> {
        let (action_set_index, _) = self.verify_action_set_existence(action_set_id)?;

        let _ = self.free_index(action_set_index);
        self.uuid_to_index.remove(&action_set_id);
        Ok(())
    }

    /// Get or create an index for an Uuid
    fn get_or_create_index(&mut self, node_id: &Uuid) -> NodeNewOrExisting {
        // Check if Uuid already has an index
        if let Some(existing) = self.uuid_to_index.get(node_id) {
            return NodeNewOrExisting::Existing(*existing);
        }

        // If there are no free nodes, create a new index, otherwise use a free node
        let index = if self.free_nodes.is_empty() {
            let index = self.next_index;
            self.next_index += 1;
            NodeNewOrExisting::New(index)
        } else {
            let index = self.free_nodes.pop().unwrap(); // validated with is_empty
            NodeNewOrExisting::New(index)
        };

        let idx = index.value();
        self.uuid_to_index.insert(*node_id, idx);
        if idx >= self.nodes.len() as u32 {
            self.nodes.resize_with((idx + 1) as usize, || NodeType::Placeholder)
        }

        index
    }

    pub fn add_parental_relationships(
        &mut self,
        child_index: u32,
        parent_index: u32,
    ) {
        self.parents.entry(child_index).or_default().insert(parent_index);
        self.children.entry(parent_index).or_default().insert(child_index);
    }

    /// Clear all relationships for an index
    fn clear_parental_relationships(&mut self, idx: u32) {

        // Clear parent references from child relationships
        if let Some(parents) = self.parents.get(&idx) {
            for &parent_idx in parents {
                self.children.entry(parent_idx).or_default().remove(&idx);
            }
        }

        // Clear child references from parent relationships
        if let Some(children) = self.children.get(&idx) {
            for &child_idx in children {
                self.parents.entry(child_idx).or_default().remove(&idx);
            }
        }

        // Clear both lists
        self.parents.remove_entry(&idx);
        self.children.remove_entry(&idx);
    }

    fn free_index(&mut self, index: u32) -> ArborResult<()> {
        if let Some(node) = self.nodes.get_mut(index as usize) {
            *node = NodeType::Placeholder;
        } else {
            return Err(ArborError::Graph(GraphError::NodeIndexNotFound(format!("Index out of bounds: {}", index))))
        }
        self.free_nodes.push(index);
        Ok(())
    }

    fn dfs_has_parent_cycle(
        &self,
        entity_index: u32,
        visited: &mut RoaringBitmap,
        current_path: &mut Vec<u32>,
    ) -> bool {
        if current_path.contains(&entity_index) {
            return true;
        }
        if visited.contains(entity_index) {
            return false;
        }

        visited.insert(entity_index);
        current_path.push(entity_index);

        if let Some(parents) = self.parents.get(&entity_index) {
            for &parent in parents {
                if self.dfs_has_parent_cycle(parent, visited, current_path)
                {
                    return true;
                }
            }
        }

        current_path.pop();
        false
    }

    fn validate_policy_relationships(
        &self,
        policy: &Policy,
    ) -> ArborResult<PolicyRelIndices> {
        // Validate principal target
        let principal_index = {
            if let Some(principal_id) = policy.principal.to_uuid() {
                self.uuid_to_index.get(&principal_id)
            } else {
                None
            }
        };
        match &policy.principal {
            PolicyTarget::Entity(entity_id) | PolicyTarget::EntityWithDescendants(entity_id) => {
                if principal_index.is_none() {
                    return Err(ArborError::Graph(GraphError::NodeNotFound(
                        entity_id.to_string(),
                    )));
                }
            }
            _ => {}
        }

        let resource_index = {
            if let Some(resource_id) = policy.resource.to_uuid() {
                self.uuid_to_index.get(&resource_id)
            } else {
                None
            }
        };
        // Validate resource target
        match &policy.resource {
            PolicyTarget::Entity(entity_id) | PolicyTarget::EntityWithDescendants(entity_id) => {
                if resource_index.is_none() {
                    return Err(ArborError::Graph(GraphError::NodeNotFound(
                        entity_id.to_string(),
                    )));
                }
            }
            _ => {}
        }

        let mut action_indices = vec![];
        // Validate actions
        for action in &policy.actions {
            if let Some(action_idx) = self.uuid_to_index.get(action) {
                action_indices.push(*action_idx);
            } else {
                return Err(ArborError::Graph(GraphError::NodeNotFound(
                    action.to_string(),
                )));
            }
        }

        let mut action_set_indicies = vec![];
        // Validate action sets
        for action_set in &policy.action_sets {
            if let Some(action_set_idx) = self.uuid_to_index.get(action_set) {
                action_set_indicies.push(*action_set_idx);
            } else {
                return Err(ArborError::Graph(GraphError::NodeNotFound(
                    action_set.to_string(),
                )));
            }
        }

        Ok((
            principal_index.copied(),
            resource_index.copied(),
            action_indices,
            action_set_indicies,
        ))
    }

    fn verify_node<T>(
        &self,
        id: Uuid,
        type_name: &str,
        extract: impl Fn(&NodeType) -> Option<&T>,
    ) -> ArborResult<(u32, &T)> {
        let node_index = self
            .uuid_to_index
            .get(&id)
            .copied()
            .ok_or_else(|| ArborError::Graph(GraphError::NodeIndexNotFound(id.to_string())))?;

        let node = self
            .nodes
            .get(node_index as usize)
            .ok_or_else(|| ArborError::Graph(GraphError::NodeNotFound(node_index.to_string())))?;

        extract(node)
            .ok_or_else(|| {
                ArborError::Graph(GraphError::TypeMismatch {
                    expected: type_name.to_string(),
                    actual: format!("{:?}", node),
                })
            })
            .map(|extracted| (node_index, extracted))
    }

    pub(crate) fn verify_entity_existence(&self, entity_id: Uuid) -> ArborResult<(u32, &Entity)> {
        self.verify_node(entity_id, "Entity", NodeType::as_entity)
    }

    fn verify_policy_existence(&self, policy_id: Uuid) -> ArborResult<(u32, &Policy)> {
        self.verify_node(policy_id, "Policy", NodeType::as_policy)
    }

    pub(crate) fn verify_action_existence(&self, action_id: Uuid) -> ArborResult<(u32, &Action)> {
        self.verify_node(action_id, "Action", NodeType::as_action)
    }

    fn verify_action_set_existence(&self, action_set_id: Uuid) -> ArborResult<(u32, &ActionSet)> {
        self.verify_node(action_set_id, "ActionSet", NodeType::as_action_set)
    }

    pub fn register_entity_type(&mut self, id: EntityTypeId, name: String) {
        self.entity_type_names.insert(id, name);
    }

    /// Look up an `EntityTypeId` by name, creating and registering one if not found.
    pub fn get_or_create_entity_type_id(&mut self, name: &str) -> EntityTypeId {
        if let Some((&id, _)) = self.entity_type_names.iter().find(|(_, v)| v.as_str() == name) {
            return id;
        }
        // Use the next available u32 (1-indexed, 0 is reserved for "unknown")
        let next_id = self.entity_type_names.len() as u32 + 1;
        let id = EntityTypeId::new(next_id);
        self.entity_type_names.insert(id, name.to_string());
        id
    }

    /// Look up an `AttributeNameId` by name, creating and registering one if not found.
    /// Mirrors `get_or_create_entity_type_id` -- same 1-indexed, linear-scan-then-append
    /// scheme, since attribute vocabularies are small and this only runs at ingestion time.
    pub fn get_or_create_attribute_name_id(&mut self, name: &str) -> AttributeNameId {
        if let Some((&id, _)) = self.attribute_names.iter().find(|(_, v)| v.as_str() == name) {
            return id;
        }
        let next_id = self.attribute_names.len() as u32 + 1;
        let id = AttributeNameId::new(next_id);
        self.attribute_names.insert(id, name.to_string());
        id
    }

    /// Upsert an entity described by an `EntityInput`, resolving the type name and any
    /// attribute paths automatically.
    pub fn upsert_entity_from_input(&mut self, input: EntityInput) -> ArborResult<()> {
        let type_id = self.get_or_create_entity_type_id(&input.type_name);

        let mut attributes = Attributes::new();
        for attr in input.attributes {
            let path: Vec<AttributeNameId> = attr
                .path
                .iter()
                .map(|segment| self.get_or_create_attribute_name_id(segment))
                .collect();
            let value = match attr.value {
                AttributeValueInput::String(s) => AttributeValue::String(s),
                AttributeValueInput::Integer(i) => AttributeValue::Integer(i),
                AttributeValueInput::Float(f) => AttributeValue::Float(f),
                AttributeValueInput::Bool(b) => AttributeValue::Bool(b),
            };
            attributes
                .set_nested(&path, value)
                .map_err(|e| ArborError::ConversionError(format!(
                    "entity {} attribute {:?}: {e}",
                    input.id, attr.path
                )))?;
        }

        let mut entity = Entity::new(input.id, input.name, type_id, input.parents);
        entity.attributes = attributes;
        self.upsert_entity(entity)
    }

    fn resolve_policy_target(&mut self, target: PolicyTargetInput) -> PolicyTarget {
        match target {
            PolicyTargetInput::Entity(id) => PolicyTarget::Entity(id),
            PolicyTargetInput::EntityWithDescendants(id) => PolicyTarget::EntityWithDescendants(id),
            PolicyTargetInput::EntityType(name) => {
                PolicyTarget::EntityType(self.get_or_create_entity_type_id(&name))
            }
            PolicyTargetInput::All => PolicyTarget::All,
        }
    }

    fn resolve_operand_input(&mut self, operand: OperandInput) -> ArborResult<Operand> {
        Ok(match operand {
            OperandInput::String(s) => Operand::String(s),
            OperandInput::Integer(i) => Operand::Integer(i),
            OperandInput::Float(f) => Operand::Float(f),
            OperandInput::Bool(b) => Operand::Bool(b),
            OperandInput::EntityRef(id) => {
                let idx = self
                    .uuid_to_index
                    .get(&id)
                    .ok_or_else(|| ArborError::Graph(GraphError::NodeNotFound(id.to_string())))?;
                Operand::EntityRef(*idx)
            }
            OperandInput::Set(items) => {
                let resolved = items
                    .into_iter()
                    .map(|item| self.resolve_operand_input(item))
                    .collect::<ArborResult<Vec<_>>>()?;
                Operand::Set(resolved)
            }
            OperandInput::Variable(scope, path) => {
                let path = path
                    .iter()
                    .map(|segment| self.get_or_create_attribute_name_id(segment))
                    .collect();
                Operand::Variable(VariableRef { scope, path })
            }
        })
    }

    /// Resolves a `ConditionInput` (raw attribute-path strings, raw entity UUIDs)
    /// into a `Condition` (interned `AttributeNameId`s, resolved entity indices),
    /// creating attribute names as needed -- same lazy-registration pattern as
    /// `resolve_policy_target`'s `EntityType` case.
    fn resolve_condition_input(&mut self, condition: ConditionInput) -> ArborResult<Condition> {
        macro_rules! binop {
            ($variant:ident, $l:expr, $r:expr) => {
                Condition::$variant(self.resolve_operand_input($l)?, self.resolve_operand_input($r)?)
            };
        }
        Ok(match condition {
            ConditionInput::Operand(op) => Condition::Operand(self.resolve_operand_input(op)?),
            ConditionInput::And(parts) => Condition::And(
                parts.into_iter().map(|c| self.resolve_condition_input(c)).collect::<ArborResult<_>>()?,
            ),
            ConditionInput::Or(parts) => Condition::Or(
                parts.into_iter().map(|c| self.resolve_condition_input(c)).collect::<ArborResult<_>>()?,
            ),
            ConditionInput::Not(inner) => Condition::Not(Box::new(self.resolve_condition_input(*inner)?)),
            ConditionInput::Eq(l, r) => binop!(Eq, l, r),
            ConditionInput::Neq(l, r) => binop!(Neq, l, r),
            ConditionInput::Lt(l, r) => binop!(Lt, l, r),
            ConditionInput::Lte(l, r) => binop!(Lte, l, r),
            ConditionInput::Gt(l, r) => binop!(Gt, l, r),
            ConditionInput::Gte(l, r) => binop!(Gte, l, r),
            ConditionInput::In(l, r) => binop!(In, l, r),
            ConditionInput::Contains(l, r) => binop!(Contains, l, r),
            ConditionInput::ContainsAll(l, r) => binop!(ContainsAll, l, r),
            ConditionInput::ContainsAny(l, r) => binop!(ContainsAny, l, r),
            ConditionInput::StartsWith(l, r) => binop!(StartsWith, l, r),
            ConditionInput::EndsWith(l, r) => binop!(EndsWith, l, r),
            ConditionInput::StringContains(l, r) => binop!(StringContains, l, r),
            ConditionInput::Like(l, r) => binop!(Like, l, r),
            ConditionInput::InHierarchy(l, r) => binop!(InHierarchy, l, r),
        })
    }

    /// Upsert a policy described by a `PolicyInput`, resolving `EntityType` target
    /// names, condition attribute paths, and `in_hierarchy` entity references
    /// automatically (creating attribute names as needed).
    pub fn upsert_policy_from_input(&mut self, input: PolicyInput) -> ArborResult<()> {
        let principal = self.resolve_policy_target(input.principal);
        let resource = self.resolve_policy_target(input.resource);
        let condition = input
            .condition
            .map(|c| self.resolve_condition_input(c))
            .transpose()?;
        let policy = Policy::new(
            input.id,
            input.name,
            None,
            input.policy_type,
            principal,
            resource,
            input.actions,
            input.action_sets,
            condition,
        );
        self.upsert_policy(policy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::Graph;
    use arbor_types::{AttributeInput, PolicyType, VariableScope};

    #[test]
    fn attribute_name_interner_is_stable_and_distinct() {
        let mut graph = Graph::new();
        let a1 = graph.get_or_create_attribute_name_id("consent_flags");
        let a2 = graph.get_or_create_attribute_name_id("age");
        let a1_again = graph.get_or_create_attribute_name_id("consent_flags");

        assert_eq!(a1, a1_again, "same name must resolve to the same id");
        assert_ne!(a1, a2, "different names must resolve to different ids");
    }

    #[test]
    fn entity_input_attributes_resolve_to_nested_attributes() {
        let mut graph = Graph::new();
        let entity_id = Uuid::new_v4();
        graph
            .upsert_entity_from_input(EntityInput {
                id: entity_id,
                name: "Carol".into(),
                type_name: "Patient".into(),
                parents: vec![],
                attributes: vec![
                    AttributeInput {
                        path: vec!["consent_flags".into(), "share_with_specialists".into()],
                        value: AttributeValueInput::Bool(true),
                    },
                    AttributeInput {
                        path: vec!["age".into()],
                        value: AttributeValueInput::Integer(42),
                    },
                ],
            })
            .expect("upsert entity with attributes");

        let age_id = graph.get_or_create_attribute_name_id("age");
        let consent_id = graph.get_or_create_attribute_name_id("consent_flags");
        let share_id = graph.get_or_create_attribute_name_id("share_with_specialists");

        let idx = *graph.uuid_to_index.get(&entity_id).expect("entity indexed");
        let entity = graph.nodes[idx as usize].as_entity().expect("is an entity");

        assert_eq!(entity.get_attribute(&age_id), Some(&AttributeValue::Integer(42)));
        assert_eq!(
            entity.get_nested_attribute(&[consent_id, share_id]),
            Some(&AttributeValue::Bool(true)),
        );
    }

    #[test]
    fn policy_condition_input_resolves_attribute_path_and_dependencies() {
        let mut graph = Graph::new();
        let patient_id = Uuid::new_v4();
        graph
            .upsert_entity_from_input(EntityInput {
                id: patient_id,
                name: "Carol".into(),
                type_name: "Patient".into(),
                parents: vec![],
                attributes: vec![],
            })
            .expect("upsert patient");

        let policy_id = Uuid::new_v4();
        graph
            .upsert_policy_from_input(PolicyInput {
                id: policy_id,
                name: "consent-gated-read".into(),
                policy_type: PolicyType::Permit,
                principal: PolicyTargetInput::All,
                resource: PolicyTargetInput::Entity(patient_id),
                actions: vec![],
                action_sets: vec![],
                condition: Some(ConditionInput::Eq(
                    OperandInput::Variable(
                        VariableScope::Resource,
                        vec!["consent_flags".into(), "share_with_specialists".into()],
                    ),
                    OperandInput::Bool(true),
                )),
            })
            .expect("upsert policy with condition");

        let consent_id = graph.get_or_create_attribute_name_id("consent_flags");
        let share_id = graph.get_or_create_attribute_name_id("share_with_specialists");

        let idx = *graph.uuid_to_index.get(&policy_id).expect("policy indexed");
        let policy = graph.nodes[idx as usize].as_policy().expect("is a policy");

        match policy.conditions.as_ref().expect("condition resolved") {
            Condition::Eq(Operand::Variable(var_ref), Operand::Bool(true)) => {
                assert_eq!(var_ref.path, vec![consent_id, share_id]);
            }
            other => panic!("unexpected resolved condition: {other:?}"),
        }

        // `Policy::new` computes dependencies automatically from the resolved
        // condition -- confirms resolution happens before dependency analysis,
        // not after (which would silently produce an empty dependency list).
        assert_eq!(
            policy.dependencies,
            vec![VariableRef { scope: VariableScope::Resource, path: vec![consent_id, share_id] }],
        );
    }

    #[test]
    fn policy_condition_input_rejects_unknown_in_hierarchy_target() {
        let mut graph = Graph::new();
        let missing_uuid = Uuid::new_v4();

        let result = graph.upsert_policy_from_input(PolicyInput {
            id: Uuid::new_v4(),
            name: "bad-hierarchy-ref".into(),
            policy_type: PolicyType::Permit,
            principal: PolicyTargetInput::All,
            resource: PolicyTargetInput::All,
            actions: vec![],
            action_sets: vec![],
            condition: Some(ConditionInput::InHierarchy(
                OperandInput::Variable(VariableScope::Principal, vec![]),
                OperandInput::EntityRef(missing_uuid),
            )),
        });

        assert!(result.is_err(), "in_hierarchy referencing an unregistered entity must fail, not silently resolve to nothing");
    }
}