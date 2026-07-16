//! Builds a real Snapshot at the given scale, archives it with rkyv, and
//! writes the lz4-compressed archive to disk, for loading via
//! `load_snapshot_rkyv`.
//!
//! Usage: dump_snapshot_rkyv <n_entities> <output_path>

use std::env;
use std::fs;

use arbor_bench::build_scenario;
use rkyv::rancor::Error;

fn main() {
    let mut args = env::args().skip(1);
    let n: usize = args
        .next()
        .expect("usage: dump_snapshot_rkyv <n_entities> <output_path>")
        .parse()
        .expect("n_entities must be a positive integer");
    let output_path = args.next().expect("usage: dump_snapshot_rkyv <n_entities> <output_path>");

    let (snapshot, _fixtures) = build_scenario(n);

    let raw = rkyv::to_bytes::<Error>(&snapshot).expect("rkyv serialize Snapshot");
    let compressed = lz4_flex::compress_prepend_size(&raw);

    fs::write(&output_path, &compressed).expect("write archive file");

    println!(
        "n_entities={n} output={output_path} raw_bytes={} raw_mb={:.1} compressed_bytes={} compressed_mb={:.1} ratio={:.2}x",
        raw.len(),
        raw.len() as f64 / (1024.0 * 1024.0),
        compressed.len(),
        compressed.len() as f64 / (1024.0 * 1024.0),
        raw.len() as f64 / compressed.len() as f64
    );
}
