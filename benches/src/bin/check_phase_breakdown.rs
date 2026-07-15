//! Manual phase-by-phase timing of check()'s exact steps (mirrors
//! engine.rs's `check()` line for line, using only public Snapshot methods),
//! to find which specific step accounts for the small reproducible
//! regression at 1M/3M scale that black-box `capacity` timing can't explain
//! (cardinality and RoaringBitmap container structure were both ruled out
//! by direct measurement). No perf/flamegraph available on this machine
//! (no dtrace permissions in this sandbox), so this is manual instrumentation
//! instead of a real profiler.
//!
//! Usage: check_phase_breakdown <n_entities>

use std::env;
use std::hint::black_box;
use std::time::Instant;

use arbor_bench::build_scenario;
use arbor_index_snapshot::Snapshot;
use roaring::RoaringBitmap;

fn main() {
    let n: usize = env::args()
        .nth(1)
        .expect("usage: check_phase_breakdown <n_entities>")
        .parse()
        .expect("n_entities must be a positive integer");

    let (snapshot, fixtures) = build_scenario(n);
    let principal_idx = fixtures.permitted_principal;
    let resource_idx = fixtures.resource;
    let action_idx = fixtures.action;

    const ITERS: u32 = 200_000;

    // Phase 1: two entity lookups.
    let start = Instant::now();
    for _ in 0..ITERS {
        let p = snapshot.get_entity(principal_idx).unwrap();
        let r = snapshot.get_entity(resource_idx).unwrap();
        black_box((p, r));
    }
    let phase1_ns = start.elapsed().as_nanos() as f64 / ITERS as f64;

    // Phase 2: action bitmap lookup.
    let start = Instant::now();
    for _ in 0..ITERS {
        let a = snapshot.get_policies_for_action(action_idx).unwrap();
        black_box(a);
    }
    let phase2_ns = start.elapsed().as_nanos() as f64 / ITERS as f64;

    // Phase 3: resolve both arena slices.
    let start = Instant::now();
    for _ in 0..ITERS {
        let p = snapshot.effective_principal_of(principal_idx);
        let r = snapshot.effective_resource_of(resource_idx);
        black_box((p, r));
    }
    let phase3_ns = start.elapsed().as_nanos() as f64 / ITERS as f64;

    // Phase 4: merge_and_filter_sorted itself.
    let action_policies = snapshot.get_policies_for_action(action_idx).unwrap();
    let start = Instant::now();
    for _ in 0..ITERS {
        let p = snapshot.effective_principal_of(principal_idx);
        let r = snapshot.effective_resource_of(resource_idx);
        let eff = Snapshot::merge_and_filter_sorted(p, r, action_policies);
        black_box(eff);
    }
    let phase4_total_ns = start.elapsed().as_nanos() as f64 / ITERS as f64;
    let phase4_merge_only_ns = phase4_total_ns - phase3_ns;

    // Direct old-vs-new comparison, same binary, same scale: rebuild the two
    // small slices as RoaringBitmaps once (as the old design stored them
    // persistently), then time the old 3-way `& & &` chain against the new
    // merge_and_filter_sorted -- holding phase5 and everything else fixed.
    let p_slice = snapshot.effective_principal_of(principal_idx);
    let r_slice = snapshot.effective_resource_of(resource_idx);
    let p_roaring = RoaringBitmap::from_sorted_iter(p_slice.iter().copied()).unwrap();
    let r_roaring = RoaringBitmap::from_sorted_iter(r_slice.iter().copied()).unwrap();

    let start = Instant::now();
    for _ in 0..ITERS {
        let mut eff = &p_roaring & &r_roaring;
        eff &= action_policies;
        black_box(eff);
    }
    let old_roaring3x_ns = start.elapsed().as_nanos() as f64 / ITERS as f64;

    let start = Instant::now();
    for _ in 0..ITERS {
        let eff = Snapshot::merge_and_filter_sorted(p_slice, r_slice, action_policies);
        black_box(eff);
    }
    let new_merge_and_filter_ns = start.elapsed().as_nanos() as f64 / ITERS as f64;

    // Phase 5: split_policy_map_for_authorization on the resulting effective set.
    let p = snapshot.effective_principal_of(principal_idx);
    let r = snapshot.effective_resource_of(resource_idx);
    let effective_policies = Snapshot::merge_and_filter_sorted(p, r, action_policies);
    let start = Instant::now();
    for _ in 0..ITERS {
        let split = snapshot.split_policy_map_for_authorization(&effective_policies);
        black_box(split);
    }
    let phase5_ns = start.elapsed().as_nanos() as f64 / ITERS as f64;

    // Full sequence, end to end, matching check() exactly (through the split).
    let start = Instant::now();
    for _ in 0..ITERS {
        let _pe = snapshot.get_entity(principal_idx).unwrap();
        let _re = snapshot.get_entity(resource_idx).unwrap();
        let ap = snapshot.get_policies_for_action(action_idx).unwrap();
        let pe = snapshot.effective_principal_of(principal_idx);
        let re = snapshot.effective_resource_of(resource_idx);
        let eff = Snapshot::merge_and_filter_sorted(pe, re, ap);
        let split = snapshot.split_policy_map_for_authorization(&eff);
        black_box(split);
    }
    let full_ns = start.elapsed().as_nanos() as f64 / ITERS as f64;

    println!("n_entities={n}");
    println!("phase1_entity_lookups_ns={phase1_ns:.2}");
    println!("phase2_action_lookup_ns={phase2_ns:.2}");
    println!("phase3_arena_resolve_ns={phase3_ns:.2}");
    println!("phase4_merge_and_filter_only_ns={phase4_merge_only_ns:.2}");
    println!("direct_comparison_old_roaring3x_ns={old_roaring3x_ns:.2}");
    println!("direct_comparison_new_merge_and_filter_ns={new_merge_and_filter_ns:.2}");
    println!("phase5_split_policy_map_ns={phase5_ns:.2}");
    println!("full_sequence_ns={full_ns:.2}");
}
