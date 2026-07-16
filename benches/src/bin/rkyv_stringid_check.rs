//! Verifies rkyv's derive handles a generic phantom-typed newtype
//! (StringId<T>(u32, PhantomData<T>)) without incorrectly inferring a
//! `T: Archive` bound -- serde needed `#[serde(bound = "")]` for this exact
//! shape; checking whether rkyv's derive needs an equivalent override or
//! infers correctly on its own.

use std::marker::PhantomData;
use rkyv::rancor::Error;
use rkyv::{Archive, Deserialize, Serialize};

// Marker type that deliberately does NOT implement Archive/Serialize/
// Deserialize, to prove StringId<Marker> doesn't require it to. Debug/
// PartialEq are only for this test's own assert_eq!, unrelated to rkyv.
#[derive(Debug, PartialEq)]
struct Marker;

#[derive(Archive, Serialize, Deserialize, Debug, PartialEq, Clone, Copy)]
struct StringId<T>(pub u32, pub PhantomData<T>);

fn main() {
    let value = StringId::<Marker>(42, PhantomData);

    let bytes = rkyv::to_bytes::<Error>(&value).expect("serialize");
    let archived = rkyv::access::<ArchivedStringId<Marker>, Error>(&bytes).expect("access/validate");
    assert_eq!(u32::from(archived.0), 42);

    let deserialized: StringId<Marker> = rkyv::deserialize::<StringId<Marker>, Error>(archived).expect("deserialize");
    assert_eq!(deserialized, value);

    println!("StringId<T> with non-Archive marker T round-tripped OK, no bound override needed");
}
