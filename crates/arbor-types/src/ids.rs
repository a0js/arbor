use std::marker::PhantomData;
use serde::{Deserialize, Serialize};
use rkyv::{Archive, Deserialize as RkyvDeserialize, Serialize as RkyvSerialize};

#[derive(Debug, PartialEq, Eq, Clone, Copy, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[derive(Archive, RkyvSerialize, RkyvDeserialize)]
pub struct EntityTypeMarker;

#[derive(Debug, PartialEq, Eq, Clone, Copy, Hash, Serialize, Deserialize)]
#[derive(Ord)]
#[derive(PartialOrd)]
#[derive(Archive, RkyvSerialize, RkyvDeserialize)]
pub struct AttributeNameMarker;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[derive(Serialize, Deserialize)]
#[serde(bound = "")]
#[derive(Archive, RkyvSerialize, RkyvDeserialize)]
pub struct StringId<T>(pub(crate) u32, #[serde(skip)] PhantomData<T>);

// Hand-written instead of `#[rkyv(derive(PartialEq, Eq, PartialOrd, Ord))]`:
// that forwards to a plain `#[derive(Ord)]` on the generated
// `ArchivedStringId<T>`, which conservatively requires `T: Ord` even though
// the actual field being compared (`PhantomData<T>`) implements `Ord`
// unconditionally in std, regardless of `T` -- same class of over-eager
// bound as the `#[serde(bound = "")]` this type already needed for the same
// reason. BTreeMap<AttributeNameId, _> (used by Attributes) needs
// ArchivedStringId: Ord to archive at all.
impl<T> PartialEq for ArchivedStringId<T> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl<T> Eq for ArchivedStringId<T> {}
impl<T> PartialOrd for ArchivedStringId<T> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl<T> Ord for ArchivedStringId<T> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.cmp(&other.0)
    }
}
impl<T> std::hash::Hash for ArchivedStringId<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        u32::from(self.0).hash(state);
    }
}

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
