use std::marker::PhantomData;

#[derive(Debug, PartialEq, Eq, Clone, Copy, Hash)]
pub struct EntityTypeMarker;

#[derive(Debug, PartialEq, Eq, Clone, Copy, Hash)]
#[derive(Ord)]
#[derive(PartialOrd)]
pub struct AttributeNameMarker;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StringId<T>(pub(crate) u32, PhantomData<T>);

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