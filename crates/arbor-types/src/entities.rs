use crate::attributes::{AttributeValue, Attributes};
use crate::ids::{AttributeNameId, EntityTypeId};
use crate::policies::IndexedPolicy;
use std::hash::Hash;
use roaring::RoaringBitmap;
use uuid::Uuid;
use serde::{Deserialize, Serialize};

/// Simplified entity descriptor for ingestion; uses a human-readable type name
/// instead of a pre-resolved `EntityTypeId`.
#[derive(Debug, Clone)]
pub struct EntityInput {
    pub id: Uuid,
    pub name: String,
    pub type_name: String,
    pub parents: Vec<Uuid>,
}

/// Represents an entity that can act as a principal, resource, or both
#[derive(Debug, Clone)]
pub struct Entity {
    pub id: Uuid,
    pub name: String,
    pub entity_type: EntityTypeId,
    pub parents: Vec<Uuid>,     // Parent entity IDs for hierarchy
    pub attributes: Attributes, // Efficient typed attributes with nesting support
}

impl PartialEq for Entity {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}
impl Eq for Entity {}
impl Hash for Entity {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

impl Entity {
    /// Create a new entity
    pub fn new(id: Uuid, name: String, entity_type: EntityTypeId, parents: Vec<Uuid>) -> Self {
        Self {
            id,
            name,
            entity_type,
            parents,
            attributes: Attributes::new(),
        }
    }

    /// Add an attribute to the entity
    pub fn add_attribute(&mut self, name: AttributeNameId, value: AttributeValue) {
        self.attributes.set(name, value);
    }

    /// Get an attribute from the entity
    pub fn get_attribute(&self, name: &AttributeNameId) -> Option<&AttributeValue> {
        self.attributes.get(name)
    }

    /// Get a nested attribute using path (e.g., ["user", "profile", "email"])
    pub fn get_nested_attribute(&self, path: &[AttributeNameId]) -> Option<&AttributeValue> {
        self.attributes.get_nested(path)
    }

    /// Set a nested attribute using path
    pub fn set_nested_attribute(
        &mut self,
        path: &[AttributeNameId],
        value: AttributeValue,
    ) -> Result<(), &'static str> {
        self.attributes.set_nested(path, value)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexedEntity {
    pub idx: u32,
    pub attributes: Attributes,
    pub entity_type: EntityTypeId,
    pub ancestors: RoaringBitmap,
    pub principal_of_policies: Option<RoaringBitmap>,
    pub resource_of_policies: Option<RoaringBitmap>,
    /// Precomputed union of all policies that apply to this entity as a principal.
    /// Set by the snapshot builder after all entities and policies are processed.
    pub effective_principal_policies: Option<RoaringBitmap>,
    /// Precomputed union of all policies that apply to this entity as a resource.
    /// Set by the snapshot builder after all entities and policies are processed.
    pub effective_resource_policies: Option<RoaringBitmap>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct IndexedEntityType {
    pub nodes_of_type: RoaringBitmap,
    pub policies_targeting_principals_of_type: RoaringBitmap,
    pub policies_targeting_resources_of_type: RoaringBitmap,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum IndexedNode {
    Entity(IndexedEntity),
    Policy(IndexedPolicy),
    Other,
}
