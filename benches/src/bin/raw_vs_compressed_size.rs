//! Reports raw (uncompressed) bincode size vs lz4-compressed size for a
//! snapshot at the given scale -- a reference point for estimating what an
//! alternative format (e.g. rkyv, which can't be lz4-compressed without
//! losing zero-copy load) would cost on disk/over the wire, since rkyv's
//! archive is closer in shape to the *uncompressed* size than the
//! compressed one bincode+lz4 currently ships.
//!
//! Usage: raw_vs_compressed_size <n_entities>

use std::env;

use arbor_bench::build_scenario;

fn mb(bytes: usize) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn main() {
    let n: usize = env::args()
        .nth(1)
        .expect("usage: raw_vs_compressed_size <n_entities>")
        .parse()
        .expect("n_entities must be a positive integer");

    let (snapshot, _fixtures) = build_scenario(n);

    let raw = bincode::serialize(&snapshot).expect("bincode serialize");
    let compressed = lz4_flex::compress_prepend_size(&raw);

    println!(
        "n_entities={n} raw_bytes={} raw_mb={:.1} compressed_bytes={} compressed_mb={:.1} ratio={:.2}x",
        raw.len(),
        mb(raw.len()),
        compressed.len(),
        mb(compressed.len()),
        raw.len() as f64 / compressed.len() as f64
    );
}
