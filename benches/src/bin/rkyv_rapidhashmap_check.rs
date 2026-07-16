//! Verifies RapidHashMap<K, V> (a type alias for std::HashMap<K, V,
//! rapidhash's RandomState>) can be used directly as a field in an
//! rkyv-derived struct with no substitution and no wrapper -- rkyv's
//! Archive/Serialize/Deserialize impls for HashMap<K, V, S> are generic
//! over any hasher S. Also confirms the archived form is a genuine
//! zero-copy hash table (real .get()/.contains_key(), not a linear scan)
//! by querying it directly off the archive without deserializing.

use rapidhash::RapidHashMap;
use rkyv::rancor::Error;
use rkyv::{Archive, Deserialize, Serialize};

#[derive(Archive, Serialize, Deserialize, Debug, PartialEq)]
struct Example {
    map: RapidHashMap<u32, u32>,
}

fn main() {
    let mut map: RapidHashMap<u32, u32> = RapidHashMap::default();
    for i in 0..10_000u32 {
        map.insert(i, i * 2);
    }
    let value = Example { map };

    let bytes = rkyv::to_bytes::<Error>(&value).expect("serialize");
    let archived = rkyv::access::<ArchivedExample, Error>(&bytes).expect("access/validate");

    // Real hash-table lookup directly on the archive -- no deserialize step.
    // Archived u32 is rkyv's explicit-endian `u32_le`, not plain u32.
    let k5000: rkyv::rend::u32_le = 5000u32.into();
    let k9999: rkyv::rend::u32_le = 9999u32.into();
    let k50000: rkyv::rend::u32_le = 50_000u32.into();
    let k1: rkyv::rend::u32_le = 1u32.into();
    assert_eq!(archived.map.get(&k5000).map(|v| u32::from(*v)), Some(10000));
    assert_eq!(archived.map.get(&k9999).map(|v| u32::from(*v)), Some(19998));
    assert!(archived.map.get(&k50000).is_none());
    assert!(archived.map.contains_key(&k1));

    let deserialized: Example = rkyv::deserialize::<Example, Error>(archived).expect("deserialize");
    assert_eq!(deserialized, value);

    println!("RapidHashMap used directly, zero-copy get() on archive OK, len={}", archived.map.len());
}
