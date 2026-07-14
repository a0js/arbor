//! Extracts real `effective_principal_policies` and `effective_resource_policies`
//! sets from a built scenario, for the arena-vs-roaring comparison on the
//! fields that actually sit on check()'s bitwise-AND hot path (unlike
//! `ancestors`, which only does point membership tests).
//!
//! Usage: dump_effective_policies <n_entities> <principal_out_path> <resource_out_path>

use std::env;

use arbor_bench::{build_scenario, write_bitmap_sets};
use arbor_types::IndexedNode;

fn main() {
    let mut args = env::args().skip(1);
    let n: usize = args
        .next()
        .expect("usage: dump_effective_policies <n_entities> <principal_out> <resource_out>")
        .parse()
        .expect("n_entities must be a positive integer");
    let principal_out = args.next().expect("missing principal_out path");
    let resource_out = args.next().expect("missing resource_out path");

    let (snapshot, _fixtures) = build_scenario(n);

    let mut principal_sets: Vec<Vec<u32>> = Vec::new();
    let mut resource_sets: Vec<Vec<u32>> = Vec::new();

    for node in &snapshot.nodes {
        if let IndexedNode::Entity(e) = node {
            principal_sets.push(
                e.effective_principal_policies
                    .as_ref()
                    .map(|b| b.iter().collect())
                    .unwrap_or_default(),
            );
            resource_sets.push(
                e.effective_resource_policies
                    .as_ref()
                    .map(|b| b.iter().collect())
                    .unwrap_or_default(),
            );
        }
    }

    let principal_nonempty = principal_sets.iter().filter(|s| !s.is_empty()).count();
    let resource_nonempty = resource_sets.iter().filter(|s| !s.is_empty()).count();
    let principal_avg_card = principal_sets.iter().map(|s| s.len()).sum::<usize>() as f64
        / principal_nonempty.max(1) as f64;
    let resource_avg_card = resource_sets.iter().map(|s| s.len()).sum::<usize>() as f64
        / resource_nonempty.max(1) as f64;

    write_bitmap_sets(&principal_out, &principal_sets);
    write_bitmap_sets(&resource_out, &resource_sets);

    println!(
        "n_entities={n} principal_nonempty={principal_nonempty} principal_avg_card={principal_avg_card:.2} \
resource_nonempty={resource_nonempty} resource_avg_card={resource_avg_card:.2}"
    );
}
