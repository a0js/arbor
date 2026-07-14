//! Verifies the full three-way intersection `check()` actually performs
//! (`effective_principal_policies & effective_resource_policies & action_policies`,
//! engine.rs:113-115) end to end, comparing:
//!
//!   current:  RoaringBitmap & RoaringBitmap & RoaringBitmap
//!   proposed: array_merge(principal_arr, resource_arr) -> small Vec,
//!             then iterate that Vec testing `.contains()` against the
//!             action_policies RoaringBitmap (which stays RoaringBitmap —
//!             action_to_policies can be large/dense, a different regime
//!             than the tiny per-entity sets).
//!
//! Usage: full_check_intersection_bench <n_entities>

use std::env;
use std::time::Instant;

use arbor_bench::build_scenario;
use arbor_types::IndexedNode;
use roaring::RoaringBitmap;

fn main() {
    let n: usize = env::args()
        .nth(1)
        .expect("usage: full_check_intersection_bench <n_entities>")
        .parse()
        .expect("n_entities must be a positive integer");

    let (snapshot, fixtures) = build_scenario(n);

    let action_bitmap = snapshot
        .get_policies_for_action(fixtures.action)
        .expect("read action must exist")
        .clone();
    let action_sorted: Vec<u32> = action_bitmap.iter().collect();
    println!("action_policies cardinality = {}", action_sorted.len());

    // Real per-entity data, same shape as the earlier effective_* experiment.
    let mut principal_roaring: Vec<RoaringBitmap> = Vec::new();
    let mut principal_arr: Vec<Vec<u32>> = Vec::new();
    let mut resource_roaring: Vec<RoaringBitmap> = Vec::new();
    let mut resource_arr: Vec<Vec<u32>> = Vec::new();

    for node in &snapshot.nodes {
        if let IndexedNode::Entity(e) = node {
            let p: Vec<u32> = e
                .effective_principal_policies
                .as_ref()
                .map(|b| b.iter().collect())
                .unwrap_or_default();
            let r: Vec<u32> = e
                .effective_resource_policies
                .as_ref()
                .map(|b| b.iter().collect())
                .unwrap_or_default();
            if !p.is_empty() {
                principal_roaring.push(RoaringBitmap::from_sorted_iter(p.iter().copied()).unwrap());
                principal_arr.push(p);
            }
            if !r.is_empty() {
                resource_roaring.push(RoaringBitmap::from_sorted_iter(r.iter().copied()).unwrap());
                resource_arr.push(r);
            }
        }
    }

    let pairs = principal_arr.len().min(resource_arr.len());
    println!("pairs={pairs}");

    const ITERS: u32 = 5;

    // --- current: roaring & roaring & roaring ---
    let mut current_total: u64 = 0;
    let start = Instant::now();
    for _ in 0..ITERS {
        for k in 0..pairs {
            let mut eff = &principal_roaring[k] & &resource_roaring[k];
            eff &= &action_bitmap;
            current_total += eff.len();
        }
    }
    let current_ns = start.elapsed().as_nanos() as f64 / (pairs as f64 * ITERS as f64);

    // --- proposed: array-merge(p, r) -> Vec, then filter via action_bitmap.contains() ---
    let mut proposed_total: u64 = 0;
    let start = Instant::now();
    for _ in 0..ITERS {
        for k in 0..pairs {
            let pa = &principal_arr[k];
            let ra = &resource_arr[k];
            let (mut i, mut j) = (0, 0);
            let mut count = 0u64;
            while i < pa.len() && j < ra.len() {
                match pa[i].cmp(&ra[j]) {
                    std::cmp::Ordering::Less => i += 1,
                    std::cmp::Ordering::Greater => j += 1,
                    std::cmp::Ordering::Equal => {
                        if action_bitmap.contains(pa[i]) {
                            count += 1;
                        }
                        i += 1;
                        j += 1;
                    }
                }
            }
            proposed_total += count;
        }
    }
    let proposed_ns = start.elapsed().as_nanos() as f64 / (pairs as f64 * ITERS as f64);

    println!(
        "current_roaring3x_ns={current_ns:.2} proposed_array_then_contains_ns={proposed_ns:.2} \
current_total={current_total} proposed_total={proposed_total}"
    );
}
