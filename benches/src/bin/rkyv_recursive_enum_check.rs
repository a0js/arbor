//! Minimal repro of the "overflow evaluating the requirement" error hit
//! porting AttributeValue/Attributes to rkyv. Neither recursion_limit
//! (tried up to 2048) nor Box/AsBox indirection fixed it -- both still
//! overflow, confirmed empirically. This is rkyv's documented, official
//! answer for genuinely recursive types (see rkyv-0.8.17/examples/
//! json_like_schema.rs, which hits the identical error for a JSON-value
//! enum): `#[rkyv(omit_bounds)]` on the recursive field tells the derive
//! macro not to auto-generate the cyclic `T: Archive` bound, paired with
//! manually supplying the non-recursive bounds it would otherwise have
//! inferred alongside it.

use std::collections::BTreeMap;
use rkyv::{Archive, Deserialize, Serialize};

#[derive(Archive, Serialize, Deserialize, Debug, PartialEq)]
#[rkyv(serialize_bounds(
    __S: rkyv::ser::Writer + rkyv::ser::Allocator,
    __S::Error: rkyv::rancor::Source,
))]
#[rkyv(deserialize_bounds(__D::Error: rkyv::rancor::Source))]
#[rkyv(bytecheck(bounds(__C: rkyv::validation::ArchiveContext)))]
enum ValueOmitBounds {
    Int(i64),
    Set(#[rkyv(omit_bounds)] Vec<ValueOmitBounds>),
    Object(#[rkyv(omit_bounds)] Wrapper),
}

#[derive(Archive, Serialize, Deserialize, Debug, PartialEq)]
struct Wrapper {
    data: BTreeMap<i64, ValueOmitBounds>,
}

fn main() {
    let value = ValueOmitBounds::Set(vec![
        ValueOmitBounds::Int(1),
        ValueOmitBounds::Object(Wrapper {
            data: BTreeMap::from([(1i64, ValueOmitBounds::Int(2))]),
        }),
    ]);

    let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&value).expect("serialize");
    let archived = rkyv::access::<ArchivedValueOmitBounds, rkyv::rancor::Error>(&bytes).expect("access");
    let deserialized: ValueOmitBounds =
        rkyv::deserialize::<ValueOmitBounds, rkyv::rancor::Error>(archived).expect("deserialize");
    assert_eq!(deserialized, value);

    println!("omit_bounds + manual bounds breaks the cycle: OK");
}
