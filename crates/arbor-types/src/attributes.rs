use crate::entities::SortedSetRef;
use crate::ids::AttributeNameId;
use chrono::{DateTime, Utc};
use ipnet::IpNet;
use std::collections::BTreeMap;
use std::net::IpAddr;
use ordered_float::OrderedFloat;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AttributeValue {
    String(String),
    Float(OrderedFloat<f64>),
    Integer(i64),
    Bool(bool),
    Timestamp(DateTime<Utc>),
    IpAddr(IpAddr),
    IpNetwork(IpNet),
    EntityRef(u32),          // Reference to another entity
    Set(Vec<AttributeValue>), // Set of attribute values
    Object(Attributes),       // Nested attributes object
}

/// The indexed/persisted counterpart of `AttributeValue`, used only by
/// `IndexedEntity::attributes`. Identical shape except `Object` and `Set`
/// recurse via a `SortedSetRef` into `Snapshot`'s shared attribute arenas
/// instead of an owned nested `Attributes`/`Vec` -- one flat arena regardless
/// of nesting depth, same reasoning as `ancestors`/`effective_*_policies`.
///
/// `Object`'s `SortedSetRef` indexes both `attribute_names_arena` and
/// `attribute_set_values_arena` in lockstep (field `k`'s name is
/// `names[offset+k]`, value is `values[offset+k]`). `Set`'s `SortedSetRef`
/// indexes `attribute_set_values_arena` only -- elements are unnamed, so the
/// corresponding `names_arena` slots are unused placeholders, keeping the
/// two arrays the same length without needing a named/unnamed split.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum IndexedAttributeValue {
    String(String),
    Float(OrderedFloat<f64>),
    Integer(i64),
    Bool(bool),
    Timestamp(DateTime<Utc>),
    IpAddr(IpAddr),
    IpNetwork(IpNet),
    EntityRef(u32),
    Set(SortedSetRef),
    Object(SortedSetRef),
}

/// Attributes wrapper that provides efficient nested object support
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Attributes {
    data: BTreeMap<AttributeNameId, AttributeValue>,
}

impl Attributes {
    /// Create a new empty attributes object
    pub fn new() -> Self {
        Self {
            data: BTreeMap::new(),
        }
    }

    /// Set an attribute value
    pub fn set(&mut self, name: AttributeNameId, value: AttributeValue) -> Option<AttributeValue> {
        self.data.insert(name, value)
    }

    /// Get an attribute value
    pub fn get(&self, name: &AttributeNameId) -> Option<&AttributeValue> {
        self.data.get(name)
    }

    /// Get a mutable reference to an attribute value
    pub fn get_mut(&mut self, name: &AttributeNameId) -> Option<&mut AttributeValue> {
        self.data.get_mut(name)
    }

    /// Remove an attribute
    pub fn remove(&mut self, name: &AttributeNameId) -> Option<AttributeValue> {
        self.data.remove(name)
    }

    /// Check if an attribute exists
    pub fn contains(&self, name: &AttributeNameId) -> bool {
        self.data.contains_key(name)
    }

    /// Get nested attribute using dot notation path
    pub fn get_nested(&self, path: &[AttributeNameId]) -> Option<&AttributeValue> {
        if path.is_empty() {
            return None;
        }

        let mut current = self.get(&path[0])?;

        for &name in &path[1..] {
            match current {
                AttributeValue::Object(attrs) => {
                    current = attrs.get(&name)?;
                }
                _ => return None,
            }
        }

        Some(current)
    }

    /// Set nested attribute using dot notation path
    pub fn set_nested(
        &mut self,
        path: &[AttributeNameId],
        value: AttributeValue,
    ) -> Result<(), &'static str> {
        if path.is_empty() {
            return Err("Empty path");
        }

        if path.len() == 1 {
            self.set(path[0], value);
            return Ok(());
        }

        // Navigate to the parent and ensure it's an object
        let parent_path = &path[..path.len() - 1];
        let target_name = path[path.len() - 1];

        // Get or create nested objects
        let mut current = self;
        for &name in parent_path {
            // Ensure current level has the nested object
            if !current.contains(&name) {
                current.set(name, AttributeValue::Object(Attributes::new()));
            }

            match current.get_mut(&name) {
                Some(AttributeValue::Object(attrs)) => {
                    current = attrs;
                }
                Some(_) => return Err("Path contains non-object value"),
                None => unreachable!(), // We just created it above
            }
        }

        current.set(target_name, value);
        Ok(())
    }

    /// Get all attribute names (non-recursive)
    pub fn keys(&self) -> impl Iterator<Item = &AttributeNameId> {
        self.data.keys()
    }

    /// Get all values (non-recursive)
    pub fn values(&self) -> impl Iterator<Item = &AttributeValue> {
        self.data.values()
    }

    /// Get all key-value pairs (non-recursive)
    pub fn iter(&self) -> impl Iterator<Item = (&AttributeNameId, &AttributeValue)> {
        self.data.iter()
    }

    /// Check if attributes is empty
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Get number of top-level attributes
    pub fn len(&self) -> usize {
        self.data.len()
    }
}

impl Default for Attributes {
    fn default() -> Self {
        Self::new()
    }
}

/// Flattens a graph-level `Attributes` (`BTreeMap`) into `pairs_arena`,
/// returning a `SortedSetRef` over the range just written. Depth-first:
/// nested `Object`/`Set` values are flattened first, so their own
/// `SortedSetRef`s are known before this range is finalized.
///
/// `Attributes::iter()` yields keys in ascending order (`BTreeMap`), so the
/// resulting arena range stays sorted by name -- required for
/// `resolve_nested_attribute`'s `binary_search_by_key`.
///
/// Shared by the real indexer (`snapshot_builder.rs`) and test fixtures that
/// need to build an arena-backed `IndexedEntity` by hand, so the two never
/// drift apart.
pub fn flatten_attributes(
    attrs: &Attributes,
    pairs_arena: &mut Vec<(AttributeNameId, IndexedAttributeValue)>,
    values_arena: &mut Vec<IndexedAttributeValue>,
) -> SortedSetRef {
    let offset = pairs_arena.len() as u32;
    for (&name, value) in attrs.iter() {
        let indexed_value = flatten_value(value, pairs_arena, values_arena);
        pairs_arena.push((name, indexed_value));
    }
    SortedSetRef { offset, len: (pairs_arena.len() as u32) - offset }
}

/// Flattens a single `AttributeValue` into its `IndexedAttributeValue`
/// counterpart, recursing into `values_arena`/`pairs_arena` for `Set`/`Object`.
pub fn flatten_value(
    value: &AttributeValue,
    pairs_arena: &mut Vec<(AttributeNameId, IndexedAttributeValue)>,
    values_arena: &mut Vec<IndexedAttributeValue>,
) -> IndexedAttributeValue {
    match value {
        AttributeValue::String(s) => IndexedAttributeValue::String(s.clone()),
        AttributeValue::Float(f) => IndexedAttributeValue::Float(*f),
        AttributeValue::Integer(i) => IndexedAttributeValue::Integer(*i),
        AttributeValue::Bool(b) => IndexedAttributeValue::Bool(*b),
        AttributeValue::Timestamp(t) => IndexedAttributeValue::Timestamp(*t),
        AttributeValue::IpAddr(ip) => IndexedAttributeValue::IpAddr(*ip),
        AttributeValue::IpNetwork(net) => IndexedAttributeValue::IpNetwork(net.clone()),
        AttributeValue::EntityRef(u) => IndexedAttributeValue::EntityRef(*u),
        AttributeValue::Set(elems) => {
            let offset = values_arena.len() as u32;
            for e in elems {
                let indexed = flatten_value(e, pairs_arena, values_arena);
                values_arena.push(indexed);
            }
            IndexedAttributeValue::Set(SortedSetRef {
                offset,
                len: (values_arena.len() as u32) - offset,
            })
        }
        AttributeValue::Object(nested) => {
            IndexedAttributeValue::Object(flatten_attributes(nested, pairs_arena, values_arena))
        }
    }
}
