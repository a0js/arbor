use roaring::RoaringBitmap;
use serde::{Deserialize, Serialize};
use crate::conditions::{Condition, VariableRef};
use crate::ids::EntityTypeId;
use uuid::Uuid;
use crate::CompiledCondition;

/// Policy types in the system
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PolicyType {
    Permit,
    Forbid,
}

/// Target specification for policies
#[derive(Debug, Clone, PartialEq)]
pub enum PolicyTarget {
    /// Specific entity
    Entity(Uuid),
    /// Entity and all descendants (for hierarchical structures)
    EntityWithDescendants(Uuid),
    /// All entities of a specific type
    EntityType(EntityTypeId),
    /// All entities
    All,
}

impl PolicyTarget {
    pub fn to_uuid(&self) -> Option<Uuid> {
        match self {
            PolicyTarget::Entity(uuid) => Some(*uuid),
            PolicyTarget::EntityWithDescendants(uuid) => Some(*uuid),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum IndexedPolicyTarget {
    Entity(u32),
    EntityWithDescendants(u32),
    EntityType(EntityTypeId),
    All
}

/// Represents a policy in the authorization system
#[derive(Debug, Clone, PartialEq)]
pub struct Policy {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub policy_type: PolicyType,
    pub principal: PolicyTarget,
    pub resource: PolicyTarget,
    pub actions: Vec<Uuid>,     // Direct Action IDs
    pub action_sets: Vec<Uuid>, // ActionSet IDs (for role-based access)
    pub conditions: Option<Condition>,
    pub dependencies: Vec<VariableRef>
}

impl Policy {
    /// Create a new policy
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: Uuid,
        name: String,
        description: Option<String>,
        policy_type: PolicyType,
        principal: PolicyTarget,
        resource: PolicyTarget,
        actions: Vec<Uuid>,
        action_sets: Vec<Uuid>,
        conditions: Option<Condition>,
    ) -> Self {
        let dependencies = conditions
            .as_ref()
            .map(|c| c.compute_dependencies())
            .unwrap_or_default();
        Self {
            id,
            name,
            description,
            policy_type,
            principal,
            resource,
            actions,
            action_sets,
            conditions,
            dependencies
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexedPolicy {
    pub idx: u32,
    pub principal_target: IndexedPolicyTarget,
    pub resource_target: IndexedPolicyTarget,
    pub actions: RoaringBitmap,
    pub conditions: Option<CompiledCondition>,
    pub is_forbidding: bool,
    pub is_conditional: bool,
}
