//! Loads an archive written by `dump_snapshot_rkyv` the way a real reader
//! would: lz4-decompress into one buffer, `rkyv::access()` (validate, no
//! allocation), then eagerly decode every `RoaringBitmap`-backed field --
//! per the "no lazy loading" decision, every bitmap gets materialized at
//! load time regardless of whether it's ever queried, and peak RSS is
//! reported via `getrusage`.
//!
//! Usage: load_snapshot_rkyv <path>

use std::env;
use std::fs;
use std::time::Instant;

use arbor_index_snapshot::ArchivedSnapshot;
use arbor_types::rkyv_with::RoaringAsBytes;
use rkyv::rancor::Error;
use rkyv::with::DeserializeWith;

fn peak_rss_bytes() -> u64 {
    unsafe {
        let mut usage: libc::rusage = std::mem::zeroed();
        libc::getrusage(libc::RUSAGE_SELF, &mut usage);
        #[cfg(target_os = "macos")]
        {
            usage.ru_maxrss as u64
        }
        #[cfg(not(target_os = "macos"))]
        {
            (usage.ru_maxrss as u64) * 1024
        }
    }
}

fn decode(archived_bytes: &rkyv::vec::ArchivedVec<u8>) -> roaring::RoaringBitmap {
    RoaringAsBytes::deserialize_with(archived_bytes, rkyv::rancor::Strategy::<_, Error>::wrap(&mut ()))
        .expect("decode RoaringBitmap")
}

fn main() {
    let path = env::args().nth(1).expect("usage: load_snapshot_rkyv <path>");

    let start = Instant::now();
    let compressed = fs::read(&path).expect("read archive file");
    let raw = lz4_flex::decompress_size_prepended(&compressed).expect("lz4 decompress");
    let archived = rkyv::access::<ArchivedSnapshot, Error>(&raw).expect("rkyv access/validate");

    // Eager decode of every RoaringBitmap-backed field -- no lazy loading.
    let mut bitmap_cardinality_sum: u64 = 0;
    let mut bitmaps_decoded: u64 = 0;

    for (_, bytes) in archived.descendants_by_target.iter() {
        bitmap_cardinality_sum += decode(bytes).len();
        bitmaps_decoded += 1;
    }
    for (_, bytes) in archived.action_to_policies.iter() {
        bitmap_cardinality_sum += decode(bytes).len();
        bitmaps_decoded += 1;
    }
    for (_, entity_type) in archived.indexed_entity_types.iter() {
        bitmap_cardinality_sum += decode(&entity_type.nodes_of_type).len();
        bitmap_cardinality_sum += decode(&entity_type.policies_targeting_principals_of_type).len();
        bitmap_cardinality_sum += decode(&entity_type.policies_targeting_resources_of_type).len();
        bitmaps_decoded += 3;
    }
    bitmap_cardinality_sum += decode(&archived.all_principal_policies).len();
    bitmap_cardinality_sum += decode(&archived.all_resource_policies).len();
    bitmap_cardinality_sum += decode(&archived.conditional_policies).len();
    bitmap_cardinality_sum += decode(&archived.forbidding_policies).len();
    bitmap_cardinality_sum += decode(&archived.descendant_principal_policies).len();
    bitmap_cardinality_sum += decode(&archived.descendant_resource_policies).len();
    bitmaps_decoded += 6;

    // Touch every node once, the way a reader scanning the snapshot would --
    // zero-copy, doesn't allocate, but exercises the archive realistically.
    let mut nodes_touched: u64 = 0;
    for node in archived.nodes.iter() {
        std::hint::black_box(node);
        nodes_touched += 1;
    }

    let load_ms = start.elapsed().as_millis();
    let rss_bytes = peak_rss_bytes();

    println!(
        "path={path} load_ms={load_ms} nodes_touched={nodes_touched} bitmaps_decoded={bitmaps_decoded} \
bitmap_cardinality_sum={bitmap_cardinality_sum} rss_bytes={rss_bytes} rss_mb={:.1}",
        rss_bytes as f64 / (1024.0 * 1024.0)
    );

    std::hint::black_box(&archived);
}
