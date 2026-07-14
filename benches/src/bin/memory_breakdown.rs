//! Diagnostic: breaks down where Snapshot bytes actually go, component by
//! component, instead of inferring it from process-level peak RSS.
//!
//! Peak RSS (as reported by `capacity`) is a high-water mark — it captures
//! transient allocations during the build (e.g. the full per-node descendant
//! table in `closures::compute_all_descendants`) even after they're dropped.
//! This binary instead sums `RoaringBitmap::serialized_size()` across the
//! *persisted* `Snapshot` fields to see what's actually resident in steady
//! state, once the transient builder state is gone.
//!
//! Usage: memory_breakdown <n_entities>

use std::env;

use arbor_bench::build_scenario;
use arbor_types::IndexedNode;

fn mb(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn main() {
    let n: usize = env::args()
        .nth(1)
        .expect("usage: memory_breakdown <n_entities>")
        .parse()
        .expect("n_entities must be a positive integer");

    let (snapshot, _fixtures) = build_scenario(n);

    let mut ancestors_bytes: u64 = 0;
    let mut principal_of_policies_bytes: u64 = 0;
    let mut resource_of_policies_bytes: u64 = 0;
    let mut effective_principal_bytes: u64 = 0;
    let mut effective_resource_bytes: u64 = 0;
    let mut entity_count: u64 = 0;
    let mut principal_of_nonempty: u64 = 0;
    let mut resource_of_nonempty: u64 = 0;
    let mut principal_of_cardinality: u64 = 0;
    let mut resource_of_cardinality: u64 = 0;

    let mut policy_count: u64 = 0;
    let mut policy_principal_desc_bytes: u64 = 0;
    let mut policy_resource_desc_bytes: u64 = 0;
    let mut policy_actions_bytes: u64 = 0;

    for node in &snapshot.nodes {
        match node {
            IndexedNode::Entity(e) => {
                entity_count += 1;
                ancestors_bytes += e.ancestors.serialized_size() as u64;
                if let Some(b) = &e.principal_of_policies {
                    principal_of_policies_bytes += b.serialized_size() as u64;
                    if !b.is_empty() {
                        principal_of_nonempty += 1;
                        principal_of_cardinality += b.len();
                    }
                }
                if let Some(b) = &e.resource_of_policies {
                    resource_of_policies_bytes += b.serialized_size() as u64;
                    if !b.is_empty() {
                        resource_of_nonempty += 1;
                        resource_of_cardinality += b.len();
                    }
                }
                if let Some(b) = &e.effective_principal_policies {
                    effective_principal_bytes += b.serialized_size() as u64;
                }
                if let Some(b) = &e.effective_resource_policies {
                    effective_resource_bytes += b.serialized_size() as u64;
                }
            }
            IndexedNode::Policy(p) => {
                policy_count += 1;
                policy_actions_bytes += p.actions.serialized_size() as u64;
                if let Some(b) = &p.principal_descendants {
                    policy_principal_desc_bytes += b.serialized_size() as u64;
                }
                if let Some(b) = &p.resource_descendants {
                    policy_resource_desc_bytes += b.serialized_size() as u64;
                }
            }
            IndexedNode::Other => {}
        }
    }

    let per_entity_struct_bytes =
        entity_count * std::mem::size_of::<arbor_types::IndexedEntity>() as u64;

    let bitmap_total = ancestors_bytes
        + principal_of_policies_bytes
        + resource_of_policies_bytes
        + effective_principal_bytes
        + effective_resource_bytes
        + policy_actions_bytes
        + policy_principal_desc_bytes
        + policy_resource_desc_bytes;

    println!("n_entities={n} entity_count={entity_count} policy_count={policy_count}");
    println!("--- per-entity bitmaps (summed across all {entity_count} entities) ---");
    println!(
        "ancestors:                  {:>10.2} MB  ({:.1} bytes/entity avg)",
        mb(ancestors_bytes),
        ancestors_bytes as f64 / entity_count as f64
    );
    println!(
        "principal_of_policies:      {:>10.2} MB  ({:.1} bytes/entity avg, {principal_of_nonempty} non-empty, avg cardinality {:.2})",
        mb(principal_of_policies_bytes),
        principal_of_policies_bytes as f64 / entity_count as f64,
        principal_of_cardinality as f64 / principal_of_nonempty.max(1) as f64
    );
    println!(
        "resource_of_policies:       {:>10.2} MB  ({:.1} bytes/entity avg, {resource_of_nonempty} non-empty, avg cardinality {:.2})",
        mb(resource_of_policies_bytes),
        resource_of_policies_bytes as f64 / entity_count as f64,
        resource_of_cardinality as f64 / resource_of_nonempty.max(1) as f64
    );
    println!(
        "effective_principal_policies:{:>9.2} MB  ({:.1} bytes/entity avg)",
        mb(effective_principal_bytes),
        effective_principal_bytes as f64 / entity_count as f64
    );
    println!(
        "effective_resource_policies:{:>10.2} MB  ({:.1} bytes/entity avg)",
        mb(effective_resource_bytes),
        effective_resource_bytes as f64 / entity_count as f64
    );
    println!("--- per-policy bitmaps (summed across all {policy_count} policies) ---");
    println!("actions:                    {:>10.2} MB", mb(policy_actions_bytes));
    println!(
        "principal_descendants:      {:>10.2} MB  (only set for EntityWithDescendants targets)",
        mb(policy_principal_desc_bytes)
    );
    println!(
        "resource_descendants:       {:>10.2} MB  (only set for EntityWithDescendants targets)",
        mb(policy_resource_desc_bytes)
    );
    println!("--- totals ---");
    println!("sum of all roaring bitmaps: {:>10.2} MB", mb(bitmap_total));
    println!(
        "raw IndexedEntity struct overhead (size_of x count, excl. bitmap heap data): {:>10.2} MB",
        mb(per_entity_struct_bytes)
    );
    println!(
        "estimated steady-state snapshot floor: {:>10.2} MB",
        mb(bitmap_total) + mb(per_entity_struct_bytes)
    );
}
