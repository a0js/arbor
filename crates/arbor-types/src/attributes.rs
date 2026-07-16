use crate::entities::SortedSetRef;
use crate::ids::AttributeNameId;
use chrono::{DateTime, Utc};
use ipnet::IpNet;
use std::collections::BTreeMap;
use std::net::IpAddr;
use ordered_float::OrderedFloat;
use serde::{Deserialize, Serialize};
use rkyv::{Archive, Deserialize as RkyvDeserialize, Serialize as RkyvSerialize};
use crate::rkyv_with::{IpNetAsBits, OrderedFloatAsF64, TimestampMillis};

// AttributeValue is genuinely self-referential (Set(Vec<Self>), and
// Object(Attributes) where Attributes holds a BTreeMap<_, Self>) -- the
// derive macro's default bound generation can't resolve that cycle (it's
// in the derive's auto-generated where-clause, not the data layout, so
// Box/with::AsBox indirection doesn't help). `omit_bounds` on the recursive
// fields plus manually supplying the non-recursive bounds it would
// otherwise have inferred is rkyv's documented answer for this shape (see
// rkyv's own examples/json_like_schema.rs, which hits the identical error
// for a JSON-value enum).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[derive(Archive, RkyvSerialize, RkyvDeserialize)]
#[rkyv(serialize_bounds(
    __S: rkyv::ser::Writer + rkyv::ser::Allocator,
    __S::Error: rkyv::rancor::Source,
))]
#[rkyv(deserialize_bounds(__D::Error: rkyv::rancor::Source))]
#[rkyv(bytecheck(bounds(__C: rkyv::validation::ArchiveContext)))]
pub enum AttributeValue {
    String(String),
    Float(#[rkyv(with = OrderedFloatAsF64)] OrderedFloat<f64>),
    Integer(i64),
    Bool(bool),
    Timestamp(#[rkyv(with = TimestampMillis)] DateTime<Utc>),
    IpAddr(IpAddr),
    IpNetwork(#[rkyv(with = IpNetAsBits)] IpNet),
    EntityRef(u32),          // Reference to another entity
    Set(#[rkyv(omit_bounds)] Vec<AttributeValue>), // Set of attribute values
    Object(#[rkyv(omit_bounds)] Attributes),       // Nested attributes object
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
#[derive(Archive, RkyvSerialize, RkyvDeserialize)]
pub enum IndexedAttributeValue {
    String(String),
    Float(#[rkyv(with = OrderedFloatAsF64)] OrderedFloat<f64>),
    Integer(i64),
    Bool(bool),
    Timestamp(#[rkyv(with = TimestampMillis)] DateTime<Utc>),
    IpAddr(IpAddr),
    IpNetwork(#[rkyv(with = IpNetAsBits)] IpNet),
    EntityRef(u32),
    Set(SortedSetRef),
    Object(SortedSetRef),
}

impl IndexedAttributeValue {
    /// Borrows into an [`crate::AttributeValueView`] -- `.as_str()` on the
    /// owned `String`, zero allocation, same as the archived counterpart's
    /// `ArchivedString` borrow.
    pub fn as_view(&self) -> crate::AttributeValueView<'_> {
        use crate::AttributeValueView as V;
        match self {
            IndexedAttributeValue::String(s) => V::String(s.as_str()),
            IndexedAttributeValue::Float(f) => V::Float(f.into_inner()),
            IndexedAttributeValue::Integer(i) => V::Integer(*i),
            IndexedAttributeValue::Bool(b) => V::Bool(*b),
            IndexedAttributeValue::Timestamp(t) => V::Timestamp(*t),
            IndexedAttributeValue::IpAddr(ip) => V::IpAddr(*ip),
            IndexedAttributeValue::IpNetwork(net) => V::IpNetwork(*net),
            IndexedAttributeValue::EntityRef(u) => V::EntityRef(*u),
            IndexedAttributeValue::Set(r) => V::Set(*r),
            IndexedAttributeValue::Object(r) => V::Object(*r),
        }
    }
}

impl ArchivedIndexedAttributeValue {
    /// Zero-copy borrow into an [`crate::AttributeValueView`] straight off
    /// the archive -- `String` borrows `ArchivedString`'s bytes directly, no
    /// allocation. Mirrors `IndexedAttributeValue::as_view`.
    pub fn as_view(&self) -> crate::AttributeValueView<'_> {
        use crate::AttributeValueView as V;
        match self {
            ArchivedIndexedAttributeValue::String(s) => V::String(s.as_str()),
            ArchivedIndexedAttributeValue::Float(f) => V::Float(f64::from(*f)),
            ArchivedIndexedAttributeValue::Integer(i) => V::Integer(i64::from(*i)),
            ArchivedIndexedAttributeValue::Bool(b) => V::Bool(*b),
            ArchivedIndexedAttributeValue::Timestamp(millis) => V::Timestamp(
                DateTime::from_timestamp_millis(i64::from(*millis))
                    .expect("millis out of DateTime<Utc> range"),
            ),
            ArchivedIndexedAttributeValue::IpAddr(ip) => {
                V::IpAddr(rkyv::deserialize::<IpAddr, rkyv::rancor::Error>(ip).expect("infallible IpAddr deserialize"))
            }
            ArchivedIndexedAttributeValue::IpNetwork(bits) => {
                V::IpNetwork(crate::rkyv_with::ipnet_from_parts(bits.v6, bits.addr, bits.prefix_len))
            }
            ArchivedIndexedAttributeValue::EntityRef(u) => V::EntityRef(u32::from(*u)),
            ArchivedIndexedAttributeValue::Set(r) => V::Set(SortedSetRef { offset: u32::from(r.offset), len: u32::from(r.len) }),
            ArchivedIndexedAttributeValue::Object(r) => V::Object(SortedSetRef { offset: u32::from(r.offset), len: u32::from(r.len) }),
        }
    }
}

/// Attributes wrapper that provides efficient nested object support
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[derive(Archive, RkyvSerialize, RkyvDeserialize)]
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
