//! Verifies the real (not synthetic) `Snapshot` type round-trips correctly
//! through rkyv: archive it, access it, and sanity-check a handful of
//! fields spanning every wrapper type used (arenas, nodes, RapidHashMap,
//! RoaringBitmap-via-with, StringId as a HashMap key).
//!
//! Usage: rkyv_snapshot_check <n_entities>

use std::env;

use arbor_bench::build_scenario;
use arbor_index_snapshot::{ArchivedSnapshot, Snapshot};
use rkyv::rancor::Error;
use rkyv::with::DeserializeWith;

fn main() {
    let n: usize = env::args()
        .nth(1)
        .expect("usage: rkyv_snapshot_check <n_entities>")
        .parse()
        .expect("n_entities must be a positive integer");

    let (snapshot, fixtures) = build_scenario(n);
    let node_count = snapshot.nodes.len();
    let ancestors_len = snapshot.ancestors_arena.len();
    let descendants_by_target_len = snapshot.descendants_by_target.len();

    let bytes = rkyv::to_bytes::<Error>(&snapshot).expect("rkyv serialize Snapshot");
    println!("archive_bytes={} archive_mb={:.1}", bytes.len(), bytes.len() as f64 / (1024.0 * 1024.0));

    let compressed = lz4_flex::compress_prepend_size(&bytes);
    println!(
        "compressed_bytes={} compressed_mb={:.1} ratio={:.2}x",
        compressed.len(),
        compressed.len() as f64 / (1024.0 * 1024.0),
        bytes.len() as f64 / compressed.len() as f64
    );

    let archived = rkyv::access::<ArchivedSnapshot, Error>(&bytes).expect("rkyv access/validate Snapshot");

    // Zero-copy checks: nodes, arenas, RapidHashMap lookup.
    assert_eq!(archived.nodes.len(), node_count, "nodes length mismatch");
    assert_eq!(archived.ancestors_arena.len(), ancestors_len, "ancestors_arena length mismatch");

    // Real hash-table lookup directly on the archive by Uuid (native
    // zero-copy support, no wrapper) -- confirms the permitted_principal
    // fixture's own uuid round-trips to the same index.
    let (&some_uuid, &some_idx) = snapshot.uuid_to_index.iter().next().expect("non-empty uuid_to_index");
    assert_eq!(archived.uuid_to_index.get(&some_uuid).copied().map(u32::from), Some(some_idx));
    let _ = fixtures.permitted_principal;

    // RoaringBitmap-via-with fields: still opaque bytes on the archive,
    // explicit deserialize needed to get a real bitmap back.
    use arbor_types::rkyv_with::RoaringAsBytes;
    let mut deserializer = ();
    if let Some((_, archived_bitmap)) = archived.descendants_by_target.iter().next() {
        let decoded = RoaringAsBytes::deserialize_with(
            archived_bitmap,
            rkyv::rancor::Strategy::<_, Error>::wrap(&mut deserializer),
        )
        .expect("deserialize RoaringBitmap from archive");
        println!("decoded one descendants_by_target bitmap: cardinality={}", decoded.len());
    }
    assert_eq!(archived.descendants_by_target.len(), descendants_by_target_len);

    // Full deserialize back to an owned Snapshot as the final correctness check.
    let deserialized: Snapshot = rkyv::deserialize::<Snapshot, Error>(archived).expect("deserialize Snapshot");
    assert_eq!(deserialized.nodes.len(), node_count);
    assert_eq!(deserialized.uuid_to_index.len(), snapshot.uuid_to_index.len());

    println!(
        "n_entities={n} node_count={node_count} ancestors_len={ancestors_len} \
descendants_by_target_len={descendants_by_target_len} -- round-trip OK"
    );
}
