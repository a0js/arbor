//! Types for condition evaluation

use serde::{Deserialize, Serialize};
use crate::entities::{IndexedEntity, SortedSetRef};
use crate::attributes::{Attributes, IndexedAttributeValue};
use crate::ids::AttributeNameId;

/// Allows the bytecode VM to look up entities by their snapshot index.
///
/// Implemented by `Snapshot` in `arbor-index-snapshot`. The trait lives in
/// `arbor-types` to avoid a circular dependency (`arbor-index-snapshot` already
/// depends on `arbor-types`).
pub trait EntityResolver {
    /// Look up an `IndexedEntity` by its snapshot index.
    fn get_entity(&self, index: u32) -> Option<&IndexedEntity>;

    /// Resolve an entity's `ancestors` (a `SortedSetRef`) into the actual
    /// sorted slice of ancestor indices. `None` if the entity doesn't exist.
    fn ancestors_of(&self, index: u32) -> Option<&[u32]>;

    /// Resolve a `SortedSetRef` for an `IndexedEntity`'s own attributes or a
    /// nested `IndexedAttributeValue::Object` into its `(name, value)` pairs,
    /// sorted by name.
    fn attribute_pairs(&self, range: SortedSetRef) -> &[(AttributeNameId, IndexedAttributeValue)];

    /// Resolve a `SortedSetRef` for an `IndexedAttributeValue::Set` into its
    /// (unnamed) elements.
    fn attribute_set_values(&self, range: SortedSetRef) -> &[IndexedAttributeValue];
}

/// Walks a nested attribute path starting from `base` (an entity's own
/// attributes, or a nested `Object`'s `SortedSetRef`), resolving through
/// `entities`'s shared attribute arena at each hop. Mirrors the old
/// `Attributes::get_nested`, but for the arena-backed indexed representation.
pub fn resolve_nested_attribute<'a>(
    entities: &'a dyn EntityResolver,
    base: SortedSetRef,
    path: &[AttributeNameId],
) -> Option<&'a IndexedAttributeValue> {
    if path.is_empty() {
        return None;
    }

    let pairs = entities.attribute_pairs(base);
    let mut current = pairs
        .binary_search_by_key(&path[0], |(k, _)| *k)
        .ok()
        .map(|i| &pairs[i].1)?;

    for &name in &path[1..] {
        match current {
            IndexedAttributeValue::Object(nested) => {
                let nested_pairs = entities.attribute_pairs(*nested);
                current = nested_pairs
                    .binary_search_by_key(&name, |(k, _)| *k)
                    .ok()
                    .map(|i| &nested_pairs[i].1)?;
            }
            _ => return None,
        }
    }

    Some(current)
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
