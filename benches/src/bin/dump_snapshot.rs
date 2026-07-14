//! Builds a scenario at the given scale and writes a packaged snapshot file
//! to disk, in the same on-disk format `AuthorizerEngine::load` reads.
//!
//! Kept separate from `load_snapshot` so the two processes' peak-RSS
//! measurements never mix: this one pays the indexer's build-time cost
//! (Graph + closures), the other only pays the authorizer's load-time cost.
//!
//! Usage: dump_snapshot <n_entities> <output_path>

use std::env;
use std::fs;

use arbor_bench::build_scenario;
use arbor_index_snapshot::PackagedSnapshot;

fn main() {
    let mut args = env::args().skip(1);
    let n: usize = args
        .next()
        .expect("usage: dump_snapshot <n_entities> <output_path>")
        .parse()
        .expect("n_entities must be a positive integer");
    let output_path = args
        .next()
        .expect("usage: dump_snapshot <n_entities> <output_path>");

    let (snapshot, _fixtures) = build_scenario(n);

    let packaged = PackagedSnapshot::from_snapshot(snapshot, 0, 0).expect("package snapshot");
    let bytes = packaged.serialize().expect("serialize packaged snapshot");

    fs::write(&output_path, &bytes).expect("write snapshot file");

    println!(
        "n_entities={n} output={output_path} file_bytes={} file_mb={:.1}",
        bytes.len(),
        bytes.len() as f64 / (1024.0 * 1024.0)
    );
}
