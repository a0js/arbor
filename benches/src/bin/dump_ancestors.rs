//! Extracts the real per-entity `ancestors` sets from a built scenario and
//! writes them to a flat scratch file, so the roaring-vs-arena comparison
//! binaries can load identical real data without re-paying the Graph/closure
//! build cost in each measurement process.
//!
//! File format: u64 entity_count, then per entity: u32 len, then `len` u32s
//! (the sorted ancestor indices), all little-endian.
//!
//! Usage: dump_ancestors <n_entities> <output_path>

use std::env;
use std::fs::File;
use std::io::{BufWriter, Write};

use arbor_bench::build_scenario;
use arbor_types::IndexedNode;

fn main() {
    let mut args = env::args().skip(1);
    let n: usize = args
        .next()
        .expect("usage: dump_ancestors <n_entities> <output_path>")
        .parse()
        .expect("n_entities must be a positive integer");
    let output_path = args
        .next()
        .expect("usage: dump_ancestors <n_entities> <output_path>");

    let (snapshot, _fixtures) = build_scenario(n);

    let ancestor_sets: Vec<Vec<u32>> = snapshot
        .nodes
        .iter()
        .enumerate()
        .filter_map(|(idx, node)| match node {
            IndexedNode::Entity(_) => Some(
                snapshot
                    .ancestors_of(idx as u32)
                    .expect("entity must resolve its own ancestors")
                    .to_vec(),
            ),
            _ => None,
        })
        .collect();

    let file = File::create(&output_path).expect("create output file");
    let mut w = BufWriter::new(file);

    w.write_all(&(ancestor_sets.len() as u64).to_le_bytes()).unwrap();
    for set in &ancestor_sets {
        w.write_all(&(set.len() as u32).to_le_bytes()).unwrap();
        for &v in set {
            w.write_all(&v.to_le_bytes()).unwrap();
        }
    }
    w.flush().unwrap();

    let total_elements: usize = ancestor_sets.iter().map(|s| s.len()).sum();
    println!(
        "n_entities={n} entity_count={} total_ancestor_elements={total_elements} avg_cardinality={:.2}",
        ancestor_sets.len(),
        total_elements as f64 / ancestor_sets.len() as f64
    );
}
