//! Diagnostic: breaks down where Snapshot bytes actually go, component by
//! component, instead of inferring it from process-level peak RSS.
//!
//! Peak RSS (as reported by `capacity`) is a high-water mark — it captures
//! transient allocations during the build (e.g. the full per-node descendant
//! table in `closures::compute_all_descendants`) even after they're dropped.
//! This binary instead sums the *persisted* `Snapshot` fields to see what's
//! actually resident in steady state, once the transient builder state is
//! gone.
//!
//! `ancestors`, `principal_of_policies`, `resource_of_policies`,
//! `effective_principal_policies` and `effective_resource_policies` are all
//! shared CSR arenas now (one `Vec<u32>` per snapshot, not one RoaringBitmap
//! per entity) -- see `SortedSetRef`. Only `action_to_policies` and the other
//! aggregate bitmaps remain `RoaringBitmap`.
//!
//! Usage: memory_breakdown <n_entities>

use std::env;

use arbor_bench::build_scenario;
use arbor_types::IndexedNode;

fn mb(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn arena_mb(arena_len: usize) -> f64 {
    mb((arena_len * std::mem::size_of::<u32>()) as u64)
}

fn main() {
    let n: usize = env::args()
        .nth(1)
        .expect("usage: memory_breakdown <n_entities>")
        .parse()
        .expect("n_entities must be a positive integer");

    let (snapshot, _fixtures) = build_scenario(n);

    let mut entity_count: u64 = 0;
    let mut principal_of_nonempty: u64 = 0;
    let mut resource_of_nonempty: u64 = 0;
    let mut principal_of_cardinality: u64 = 0;
    let mut resource_of_cardinality: u64 = 0;
    let mut eff_principal_nonempty: u64 = 0;
    let mut eff_resource_nonempty: u64 = 0;
    let mut eff_principal_cardinality: u64 = 0;
    let mut eff_resource_cardinality: u64 = 0;
    let mut eff_principal_max: u32 = 0;
    let mut eff_resource_max: u32 = 0;

    let mut policy_count: u64 = 0;

    for node in &snapshot.nodes {
        match node {
            IndexedNode::Entity(e) => {
                entity_count += 1;
                if let Some(r) = e.principal_of_policies {
                    if !r.is_empty() {
                        principal_of_nonempty += 1;
                        principal_of_cardinality += r.len as u64;
                    }
                }
                if let Some(r) = e.resource_of_policies {
                    if !r.is_empty() {
                        resource_of_nonempty += 1;
                        resource_of_cardinality += r.len as u64;
                    }
                }
                if let Some(r) = e.effective_principal_policies {
                    if !r.is_empty() {
                        eff_principal_nonempty += 1;
                        eff_principal_cardinality += r.len as u64;
                        eff_principal_max = eff_principal_max.max(r.len);
                    }
                }
                if let Some(r) = e.effective_resource_policies {
                    if !r.is_empty() {
                        eff_resource_nonempty += 1;
                        eff_resource_cardinality += r.len as u64;
                        eff_resource_max = eff_resource_max.max(r.len);
                    }
                }
            }
            IndexedNode::Policy(_) => {
                policy_count += 1;
            }
            IndexedNode::Other => {}
        }
    }

    let per_entity_struct_bytes =
        entity_count * std::mem::size_of::<arbor_types::IndexedEntity>() as u64;

    let arena_total_mb = arena_mb(snapshot.ancestors_arena.len())
        + arena_mb(snapshot.principal_of_arena.len())
        + arena_mb(snapshot.resource_of_arena.len())
        + arena_mb(snapshot.effective_principal_arena.len())
        + arena_mb(snapshot.effective_resource_arena.len());

    let descendants_by_target_bytes: u64 = snapshot
        .descendants_by_target
        .values()
        .map(|b| b.serialized_size() as u64)
        .sum();
    let policy_bitmap_mb = mb(descendants_by_target_bytes);

    println!("n_entities={n} entity_count={entity_count} policy_count={policy_count}");
    println!("--- per-entity CSR arenas (one per snapshot, not one per entity) ---");
    println!(
        "ancestors:                   {:>10.2} MB  ({:.1} bytes/entity avg)",
        arena_mb(snapshot.ancestors_arena.len()),
        (snapshot.ancestors_arena.len() * 4) as f64 / entity_count as f64
    );
    println!(
        "principal_of_policies:       {:>10.2} MB  ({} non-empty, avg cardinality {:.2})",
        arena_mb(snapshot.principal_of_arena.len()),
        principal_of_nonempty,
        principal_of_cardinality as f64 / principal_of_nonempty.max(1) as f64
    );
    println!(
        "resource_of_policies:        {:>10.2} MB  ({} non-empty, avg cardinality {:.2})",
        arena_mb(snapshot.resource_of_arena.len()),
        resource_of_nonempty,
        resource_of_cardinality as f64 / resource_of_nonempty.max(1) as f64
    );
    println!(
        "effective_principal_policies:{:>10.2} MB  ({} non-empty, avg cardinality {:.2}, max {})",
        arena_mb(snapshot.effective_principal_arena.len()),
        eff_principal_nonempty,
        eff_principal_cardinality as f64 / eff_principal_nonempty.max(1) as f64,
        eff_principal_max
    );
    println!(
        "effective_resource_policies: {:>10.2} MB  ({} non-empty, avg cardinality {:.2}, max {})",
        arena_mb(snapshot.effective_resource_arena.len()),
        eff_resource_nonempty,
        eff_resource_cardinality as f64 / eff_resource_nonempty.max(1) as f64,
        eff_resource_max
    );
    println!(
        "--- descendants_by_target (deduplicated across all {policy_count} policies, still RoaringBitmap) ---"
    );
    println!(
        "descendants_by_target:      {:>10.2} MB  ({} distinct EntityWithDescendants targets)",
        mb(descendants_by_target_bytes),
        snapshot.descendants_by_target.len()
    );
    println!("--- totals ---");
    println!("sum of all CSR arenas + policy bitmaps: {:>10.2} MB", arena_total_mb + policy_bitmap_mb);
    println!(
        "raw IndexedEntity struct overhead (size_of x count, excl. arena data): {:>10.2} MB",
        mb(per_entity_struct_bytes)
    );
    println!(
        "estimated steady-state snapshot floor: {:>10.2} MB",
        arena_total_mb + policy_bitmap_mb + mb(per_entity_struct_bytes)
    );
}
