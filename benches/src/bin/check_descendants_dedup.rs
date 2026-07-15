//! Compares total `EntityWithDescendants` target *references* across all
//! policies (what the OLD per-policy-cloned-bitmap scheme would have had to
//! deserialize -- one `RoaringBitmap::deserialize_from` call each) against
//! the number of *distinct* targets in `descendants_by_target` (what the
//! NEW deduplicated scheme actually deserializes). The gap between the two
//! is the number of `RoaringBitmap::deserialize_from` calls -- and their
//! container allocations -- eliminated by deduplication.
//!
//! Usage: check_descendants_dedup <path>

use std::env;
use std::path::Path;

use arbor_authorizer::engine::AuthorizerEngine;
use arbor_types::{IndexedNode, IndexedPolicyTarget};

fn main() {
    let path = env::args().nth(1).expect("usage: check_descendants_dedup <path>");
    let engine = AuthorizerEngine::load(Path::new(&path)).expect("load snapshot");
    let snap = engine.snapshot();

    let mut total_references = 0u64;
    for node in &snap.nodes {
        if let IndexedNode::Policy(p) = node {
            if matches!(p.principal_target, IndexedPolicyTarget::EntityWithDescendants(_)) {
                total_references += 1;
            }
            if matches!(p.resource_target, IndexedPolicyTarget::EntityWithDescendants(_)) {
                total_references += 1;
            }
        }
    }

    let distinct_targets = snap.descendants_by_target.len() as u64;

    println!(
        "total_EntityWithDescendants_references={total_references} distinct_targets={distinct_targets} \
eliminated_deserialize_calls={} dedup_ratio={:.2}x",
        total_references.saturating_sub(distinct_targets),
        total_references as f64 / distinct_targets.max(1) as f64
    );
}
