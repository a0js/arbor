//! Verifies the `RoaringBitmap` `with`-wrapper: unlike `Uuid`/`DateTime`/
//! `IpNet`, this one is NOT meant to give zero-copy reads of a real
//! `RoaringBitmap` -- the archived field is just opaque bytes (`roaring`'s
//! own `serialize_into` format, the same bytes bincode already stores
//! today). Getting a real, usable `RoaringBitmap` back always means calling
//! `RoaringBitmap::deserialize_from()`, which allocates -- consistent with
//! the "no lazy loading" decision: every `RoaringBitmap`-backed field still
//! gets eagerly deserialized at load time, same cost as today, `rkyv` or
//! not. This check demonstrates both halves: the archived form really is
//! just raw bytes (no bitmap semantics available without deserializing),
//! and the deserialize step really does reconstruct a correct bitmap.

use rkyv::rancor::Error;
use rkyv::ser::{Allocator, Writer};
use rkyv::vec::{ArchivedVec, VecResolver};
use rkyv::with::{ArchiveWith, DeserializeWith, SerializeWith};
use rkyv::{Place, Serialize, Archive as ArchiveTrait, Deserialize as DeserializeTrait};
use roaring::RoaringBitmap;

pub struct RoaringAsBytes;

impl ArchiveWith<RoaringBitmap> for RoaringAsBytes {
    type Archived = ArchivedVec<u8>;
    type Resolver = VecResolver;

    fn resolve_with(field: &RoaringBitmap, resolver: Self::Resolver, out: Place<Self::Archived>) {
        let mut buf = Vec::new();
        field.serialize_into(&mut buf).expect("roaring serialize");
        ArchivedVec::resolve_from_slice(&buf, resolver, out);
    }
}

impl<S> SerializeWith<RoaringBitmap, S> for RoaringAsBytes
where
    S: rkyv::rancor::Fallible + Allocator + Writer + ?Sized,
{
    fn serialize_with(field: &RoaringBitmap, serializer: &mut S) -> Result<Self::Resolver, S::Error> {
        let mut buf = Vec::new();
        field.serialize_into(&mut buf).expect("roaring serialize");
        ArchivedVec::serialize_from_slice(&buf, serializer)
    }
}

impl<D> DeserializeWith<ArchivedVec<u8>, RoaringBitmap, D> for RoaringAsBytes
where
    D: rkyv::rancor::Fallible + ?Sized,
{
    fn deserialize_with(field: &ArchivedVec<u8>, _deserializer: &mut D) -> Result<RoaringBitmap, D::Error> {
        Ok(RoaringBitmap::deserialize_from(field.as_slice()).expect("roaring deserialize"))
    }
}

#[derive(ArchiveTrait, Serialize, DeserializeTrait, Debug, PartialEq)]
struct Example {
    idx: u32,
    #[rkyv(with = RoaringAsBytes)]
    descendants: RoaringBitmap,
}

fn main() {
    let mut bitmap = RoaringBitmap::new();
    bitmap.insert_range(0..5000);
    bitmap.insert(1_000_000);
    let value = Example { idx: 42, descendants: bitmap };

    let bytes = rkyv::to_bytes::<Error>(&value).expect("serialize");
    let archived = rkyv::access::<ArchivedExample, Error>(&bytes).expect("access/validate");

    // The archived field is just opaque bytes -- no bitmap semantics
    // available without an explicit deserialize step. This length is
    // roaring's serialized byte count, not the bitmap's cardinality.
    println!(
        "archived.descendants is raw bytes: {} bytes (NOT the same as cardinality {})",
        archived.descendants.len(),
        value.descendants.len()
    );

    // Real bitmap semantics require the explicit eager-decode step.
    let decoded: RoaringBitmap =
        RoaringAsBytes::deserialize_with(&archived.descendants, rkyv::rancor::Strategy::<_, Error>::wrap(&mut ()))
            .expect("deserialize_with");
    assert_eq!(decoded, value.descendants);
    assert!(decoded.contains(2500));
    assert!(decoded.contains(1_000_000));
    assert!(!decoded.contains(5000));

    // Full struct deserialize also goes through the same wrapper.
    let deserialized: Example = rkyv::deserialize::<Example, Error>(archived).expect("deserialize");
    assert_eq!(deserialized, value);

    println!("roaring wrapper round-trip OK: cardinality={}", deserialized.descendants.len());
}
