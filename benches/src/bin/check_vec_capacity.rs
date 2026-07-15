//! Verifies whether `reserved_vec_serde` actually eliminated the `RawVec`
//! doubling seen in `malloc_history` -- prints capacity vs len for each of
//! the six fields switched to the custom deserializer. If capacity == len
//! (no slop), the fix took; if capacity is ~2x len, it's still doubling.
//!
//! Usage: check_vec_capacity <path>

use std::env;
use std::path::Path;

use arbor_authorizer::engine::AuthorizerEngine;

fn report(name: &str, len: usize, cap: usize, elem_size: usize) {
    let slop_bytes = (cap - len) * elem_size;
    println!(
        "{name:<28} len={len:>10} cap={cap:>10} slop={:>6.1}%  wasted={:.2} MB",
        (cap as f64 / len.max(1) as f64 - 1.0) * 100.0,
        slop_bytes as f64 / (1024.0 * 1024.0)
    );
}

fn main() {
    let path = env::args().nth(1).expect("usage: check_vec_capacity <path>");
    let engine = AuthorizerEngine::load(Path::new(&path)).expect("load snapshot");
    let snap = engine.snapshot();

    report("nodes", snap.nodes.len(), snap.nodes.capacity(), std::mem::size_of::<arbor_types::IndexedNode>());
    report("ancestors_arena", snap.ancestors_arena.len(), snap.ancestors_arena.capacity(), 4);
    report("principal_of_arena", snap.principal_of_arena.len(), snap.principal_of_arena.capacity(), 4);
    report("resource_of_arena", snap.resource_of_arena.len(), snap.resource_of_arena.capacity(), 4);
    report("effective_principal_arena", snap.effective_principal_arena.len(), snap.effective_principal_arena.capacity(), 4);
    report("effective_resource_arena", snap.effective_resource_arena.len(), snap.effective_resource_arena.capacity(), 4);
    report(
        "attribute_pairs_arena",
        snap.attribute_pairs_arena.len(),
        snap.attribute_pairs_arena.capacity(),
        std::mem::size_of::<(arbor_types::AttributeNameId, arbor_types::IndexedAttributeValue)>(),
    );
    report(
        "attribute_set_values_arena",
        snap.attribute_set_values_arena.len(),
        snap.attribute_set_values_arena.capacity(),
        std::mem::size_of::<arbor_types::IndexedAttributeValue>(),
    );
}
