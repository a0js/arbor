use crate::ids::AttributeNameId;
use chrono::{DateTime, Utc};
use ipnet::IpNet;
use std::collections::BTreeMap;
use std::net::IpAddr;
use ordered_float::OrderedFloat;

#[derive(Debug, Clone, PartialEq)]
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

/// Attributes wrapper that provides efficient nested object support
#[derive(Debug, Clone, PartialEq)]
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
