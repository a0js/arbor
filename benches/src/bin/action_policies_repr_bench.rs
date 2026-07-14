//! Tests whether RoaringBitmap actually earns its keep on `action_to_policies`
//! — the field explicitly left untouched in tasks #1-#3 on the assumption
//! that its larger cardinality (thousands of policies per action, vs ~6-18
//! per entity) is where Roaring's compression and container-level ops start
//! to beat a flat sorted array. This runs the same contains()/intersection
//! comparison used for `ancestors` and `effective_*_policies`, but at the
//! real `action_to_policies` scale instead of assuming the hypothesis holds.
//!
//! Usage: action_policies_repr_bench <n_entities>

use std::env;
use std::time::Instant;

use arbor_bench::build_scenario;
use roaring::RoaringBitmap;

fn main() {
    let n: usize = env::args()
        .nth(1)
        .expect("usage: action_policies_repr_bench <n_entities>")
        .parse()
        .expect("n_entities must be a positive integer");

    let (snapshot, fixtures) = build_scenario(n);

    let action_bitmap = snapshot
        .get_policies_for_action(fixtures.action)
        .expect("read action must exist")
        .clone();
    let full: Vec<u32> = action_bitmap.iter().collect();
    let cardinality = full.len();

    // --- memory: serialized size, roaring vs flat array ---
    let roaring_bytes = action_bitmap.serialized_size();
    let array_bytes = cardinality * std::mem::size_of::<u32>();
    println!(
        "cardinality={cardinality} roaring_serialized_bytes={roaring_bytes} \
flat_array_bytes={array_bytes} (this field is one-per-action, not one-per-entity, \
so absolute memory impact is bounded regardless of representation)"
    );

    // --- contains(): point membership at this cardinality ---
    const CONTAINS_ITERS: u32 = 200_000;
    let mut hits = 0u64;
    let start = Instant::now();
    for i in 0..CONTAINS_ITERS {
        let probe = full[(i as usize * 7) % full.len()];
        if action_bitmap.contains(probe) {
            hits += 1;
        }
    }
    let roaring_contains_ns = start.elapsed().as_nanos() as f64 / CONTAINS_ITERS as f64;

    let mut array_hits = 0u64;
    let start = Instant::now();
    for i in 0..CONTAINS_ITERS {
        let probe = full[(i as usize * 7) % full.len()];
        if full.binary_search(&probe).is_ok() {
            array_hits += 1;
        }
    }
    let array_contains_ns = start.elapsed().as_nanos() as f64 / CONTAINS_ITERS as f64;

    println!(
        "contains: roaring_ns={roaring_contains_ns:.2} array_binary_search_ns={array_contains_ns:.2} \
roaring_hits={hits} array_hits={array_hits}"
    );

    // --- large-set intersection: split the real set into two halves by
    // position parity, so both sides are real subsets of real data at
    // roughly half the full cardinality each. ---
    let half_a: Vec<u32> = full.iter().step_by(2).copied().collect();
    let half_b: Vec<u32> = full.iter().skip(1).step_by(2).copied().collect();
    let roaring_a = RoaringBitmap::from_sorted_iter(half_a.iter().copied()).unwrap();
    let roaring_b = RoaringBitmap::from_sorted_iter(half_b.iter().copied()).unwrap();

    const INTERSECT_ITERS: u32 = 2_000;
    let mut roaring_total: u64 = 0;
    let start = Instant::now();
    for _ in 0..INTERSECT_ITERS {
        let inter = &roaring_a & &roaring_b;
        roaring_total += inter.len();
    }
    let roaring_intersect_ns = start.elapsed().as_nanos() as f64 / INTERSECT_ITERS as f64;

    let mut array_total: u64 = 0;
    let start = Instant::now();
    for _ in 0..INTERSECT_ITERS {
        let (mut i, mut j) = (0, 0);
        let mut count = 0u64;
        while i < half_a.len() && j < half_b.len() {
            match half_a[i].cmp(&half_b[j]) {
                std::cmp::Ordering::Less => i += 1,
                std::cmp::Ordering::Greater => j += 1,
                std::cmp::Ordering::Equal => {
                    count += 1;
                    i += 1;
                    j += 1;
                }
            }
        }
        array_total += count;
    }
    let array_intersect_ns = start.elapsed().as_nanos() as f64 / INTERSECT_ITERS as f64;

    println!(
        "intersect (halves, {} vs {} elements): roaring_ns={roaring_intersect_ns:.2} \
array_merge_ns={array_intersect_ns:.2} roaring_total={roaring_total} array_total={array_total}",
        half_a.len(),
        half_b.len()
    );
}
