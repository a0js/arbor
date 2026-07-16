//! Types for condition evaluation

use chrono::{DateTime, Utc};
use ipnet::IpNet;
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use crate::entities::{IndexedEntity, SortedSetRef};
use crate::attributes::Attributes;
use crate::ids::AttributeNameId;

/// Borrowed view over an attribute value, returned by [`EntityResolver`]
/// instead of a reference to the owned `IndexedAttributeValue`/
/// `ArchivedIndexedAttributeValue` directly -- one concrete enum both the
/// in-memory `Snapshot` and the rkyv-backed reader can produce cheaply
/// (`String(&str)` borrows either an owned `String` or a zero-copy
/// `ArchivedString`, no allocation either way), keeping `EntityResolver`
/// object-safe without materializing owned attribute data at load time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AttributeValueView<'a> {
    String(&'a str),
    Float(f64),
    Integer(i64),
    Bool(bool),
    Timestamp(DateTime<Utc>),
    IpAddr(IpAddr),
    IpNetwork(IpNet),
    EntityRef(u32),
    Set(SortedSetRef),
    Object(SortedSetRef),
}

/// Allows the bytecode VM to look up entities by their snapshot index.
///
/// Implemented by `Snapshot` and the rkyv-backed reader in
/// `arbor-index-snapshot`. The trait lives in `arbor-types` to avoid a
/// circular dependency (`arbor-index-snapshot` already depends on
/// `arbor-types`).
pub trait EntityResolver {
    /// Look up an `IndexedEntity` by its snapshot index.
    fn get_entity(&self, index: u32) -> Option<&IndexedEntity>;

    /// Resolve an entity's `ancestors` (a `SortedSetRef`) into the actual
    /// sorted slice of ancestor indices. `None` if the entity doesn't exist.
    fn ancestors_of(&self, index: u32) -> Option<&[u32]>;

    /// Walks a nested attribute path starting from `base` (an entity's own
    /// attributes, or a nested `Object`'s `SortedSetRef`), binary-searching
    /// by name at each hop. Resolved internally per backing store since the
    /// concrete `(name, value)` pair representation differs (owned vs.
    /// archived), so the pair slice's concrete element type never has to
    /// cross the trait boundary.
    fn resolve_attribute_path(&self, base: SortedSetRef, path: &[AttributeNameId]) -> Option<AttributeValueView<'_>>;

    /// Resolve a `SortedSetRef` for an `IndexedAttributeValue::Set` into its
    /// (unnamed) elements. Small, bounded by one Set attribute's element
    /// count, and only paid when a condition actually reads a Set-typed
    /// attribute during evaluation -- not a load-time cost.
    fn attribute_set_values(&self, range: SortedSetRef) -> Vec<AttributeValueView<'_>>;

    /// Resolve a `SortedSetRef` for an `IndexedAttributeValue::Object` into
    /// all of its `(name, value)` pairs -- used to materialize a whole
    /// nested object (e.g. an `Object` found inside a `Set`), as opposed to
    /// `resolve_attribute_path`'s single named lookup. Same bounded,
    /// pay-per-use cost as `attribute_set_values`.
    fn attribute_pairs_view(&self, range: SortedSetRef) -> Vec<(AttributeNameId, AttributeValueView<'_>)>;
}

/// Result of evaluating a condition
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ConditionResult {
    /// Condition evaluated to true
    True,
    /// Condition evaluated to false
    False,
    /// Invalid operation, type error, or compiler invariant violation
    Invalid(Vec<EvaluationError>),
}

/// Errors that can occur during condition evaluation
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum EvaluationError {
    /// An entity reference is not found
    MissingEntity { entity_index: u32 },
    /// Cannot compare these scalar types
    InvalidScalarComparison { message: String },
    /// Invalid types for an operation
    InvalidTypes { message: String },
    /// Feature not yet implemented
    Unimplemented(String),
    /// Stack underflow during bytecode execution
    StackUnderflow,
    /// Other execution error
    ExecutionError(String),
}

/// Context for evaluating conditions
pub struct EvaluationContext<'a> {
    /// Principal entity with attributes and hierarchy bitmaps
    pub principal: &'a IndexedEntity,
    /// Resource entity with attributes and hierarchy bitmaps
    pub resource: &'a IndexedEntity,
    /// Optional context attributes (e.g., time, IP, custom data)
    pub context_attrs: Option<&'a Attributes>,
    /// Entity resolver for looking up arbitrary entities by index or UUID.
    /// Required when evaluating `InHierarchyVar`. None in contexts where
    /// sub-entity hierarchy checks are not needed (e.g., unit tests).
    pub entities: &'a dyn EntityResolver,
}

impl<'a> EvaluationContext<'a> {
    pub fn new(
        principal: &'a IndexedEntity,
        resource: &'a IndexedEntity,
        context_attrs: Option<&'a Attributes>,
        entities: &'a dyn EntityResolver,
    ) -> Self {
        Self { principal, resource, context_attrs, entities }
    }
}
