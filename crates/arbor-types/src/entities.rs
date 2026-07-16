use crate::attributes::{AttributeValue, Attributes};
use crate::ids::{AttributeNameId, EntityTypeId};
use crate::policies::IndexedPolicy;
use std::hash::Hash;
use roaring::RoaringBitmap;
use uuid::Uuid;
use serde::{Deserialize, Serialize};
use rkyv::{Archive, Deserialize as RkyvDeserialize, Serialize as RkyvSerialize};
use crate::rkyv_with::RoaringAsBytes;

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

/// A slice into a shared, sorted `u32` arena — `offset`/`len` index into a
/// `Vec<u32>` owned elsewhere (the `Snapshot`'s per-field arena).
///
/// Used in place of `RoaringBitmap` for per-entity sets small enough (tens of
/// elements, scattered across up to millions of possible indices) that
/// Roaring's container machinery is pure allocation overhead with no
/// compression benefit. See `ancestors_arena` on `Snapshot`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[derive(Archive, RkyvSerialize, RkyvDeserialize)]
pub struct SortedSetRef {
    pub offset: u32,
    pub len: u32,
}

impl SortedSetRef {
    pub const EMPTY: Self = Self { offset: 0, len: 0 };

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[derive(Archive, RkyvSerialize, RkyvDeserialize)]
pub struct IndexedEntity {
    pub idx: u32,
    /// The entity's own attribute set -- a `SortedSetRef` into
    /// `Snapshot::attribute_pairs_arena`, i.e. it's treated identically to a
    /// nested `IndexedAttributeValue::Object`.
    pub attributes: SortedSetRef,
    pub entity_type: EntityTypeId,
    pub ancestors: SortedSetRef,
    pub principal_of_policies: Option<SortedSetRef>,
    pub resource_of_policies: Option<SortedSetRef>,
    /// Precomputed union of all policies that apply to this entity as a principal.
    /// Set by the snapshot builder after all entities and policies are processed.
    pub effective_principal_policies: Option<SortedSetRef>,
    /// Precomputed union of all policies that apply to this entity as a resource.
    /// Set by the snapshot builder after all entities and policies are processed.
    pub effective_resource_policies: Option<SortedSetRef>,
}

#[derive(Debug, Serialize, Deserialize)]
#[derive(Archive, RkyvSerialize, RkyvDeserialize)]
pub struct IndexedEntityType {
    #[rkyv(with = RoaringAsBytes)]
    pub nodes_of_type: RoaringBitmap,
    #[rkyv(with = RoaringAsBytes)]
    pub policies_targeting_principals_of_type: RoaringBitmap,
    #[rkyv(with = RoaringAsBytes)]
    pub policies_targeting_resources_of_type: RoaringBitmap,
}

#[derive(Debug, Serialize, Deserialize)]
#[derive(Archive, RkyvSerialize, RkyvDeserialize)]
pub enum IndexedNode {
    Entity(IndexedEntity),
    /// Boxed because `IndexedPolicy` (144 bytes -- several inline
    /// `RoaringBitmap`s) is roughly 2x the size of `IndexedEntity` (72
    /// bytes). Without boxing, the enum sizes to its largest variant, so
    /// every `Entity` node -- millions of them -- would pay for space only
    /// the much rarer `Policy` variant needs. `Box` is pointer-sized
    /// regardless of the boxed type, so this nearly halves every entity
    /// node's footprint at the cost of one extra pointer hop on the rare
    /// path that reads a full policy.
    Policy(Box<IndexedPolicy>),
    Other,
}
