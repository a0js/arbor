use std::marker::PhantomData;
use serde::{Deserialize, Serialize};

#[derive(Debug, PartialEq, Eq, Clone, Copy, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct EntityTypeMarker;

#[derive(Debug, PartialEq, Eq, Clone, Copy, Hash, Serialize, Deserialize)]
#[derive(Ord)]
#[derive(PartialOrd)]
pub struct AttributeNameMarker;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[derive(Serialize, Deserialize)]
#[serde(bound = "")]
pub struct StringId<T>(pub(crate) u32, #[serde(skip)] PhantomData<T>);

impl<T> StringId<T> {
    /// Create a new string ID
    pub fn new(id: u32) -> Self {
        Self(id, PhantomData)
    }

    /// Get the underlying ID
    pub fn as_u32(&self) -> u32 {
        self.0
    }
}

pub type EntityTypeId = StringId<EntityTypeMarker>;
pub type AttributeNameId = StringId<AttributeNameMarker>;
