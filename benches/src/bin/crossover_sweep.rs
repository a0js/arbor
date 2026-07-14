//! Sweeps *density within a single 65536-element chunk* — not total
//! cardinality across a wide range — to find where RoaringBitmap actually
//! starts beating a flat sorted array on contains()/intersection().
//!
//! First attempt at this got the independent variable wrong: it spread
//! `cardinality` elements across a 0..1_000_000 range, so even at 32,000
//! elements, only ~2,114 fell into any single 65536-slot chunk — under the
//! crate's internal ARRAY_LIMIT (4096, confirmed in container.rs), so
//! Roaring never switched to its bitmap-container mode and the array won
//! everywhere. The real `action_policies` case has 12,000 elements packed
//! into a ~13,336-wide range (90% density in one chunk) — that's what
//! actually triggers the container switch. This version varies density
//! within one chunk directly to find the real crossover.
//!
//! Usage: crossover_sweep [cardinalities...]  (defaults to a standard sweep,
//! all packed into a single 65536-wide chunk)

use std::env;
use std::time::Instant;

use roaring::RoaringBitmap;

/// `cardinality` values densely packed into a single 65536-wide chunk,
/// evenly spaced within it — matches how `action_policies` looks (90%
/// density within one chunk), unlike spreading over a huge range.
fn make_set(cardinality: usize, offset: u32) -> Vec<u32> {
    const CHUNK: u32 = 65536;
    let stride = (CHUNK / cardinality.max(1) as u32).max(1);
    (0..cardinality as u32).map(|i| offset + i * stride).collect()
}

fn bench_one(cardinality: usize) {
    let a = make_set(cardinality, 0);
    let b = make_set(cardinality, 1); // offset by 1: interleaved, mostly disjoint, same chunk

    let roaring_a = RoaringBitmap::from_sorted_iter(a.iter().copied()).unwrap();
    let roaring_b = RoaringBitmap::from_sorted_iter(b.iter().copied()).unwrap();

    // contains()
    const CONTAINS_ITERS: usize = 100_000;
    let mut hits = 0u64;
    let start = Instant::now();
    for i in 0..CONTAINS_ITERS {
        let probe = a[i % a.len()];
        if roaring_a.contains(probe) {
            hits += 1;
        }
    }
    let roaring_contains_ns = start.elapsed().as_nanos() as f64 / CONTAINS_ITERS as f64;

    let mut array_hits = 0u64;
    let start = Instant::now();
    for i in 0..CONTAINS_ITERS {
        let probe = a[i % a.len()];
        if a.binary_search(&probe).is_ok() {
            array_hits += 1;
        }
    }
    let array_contains_ns = start.elapsed().as_nanos() as f64 / CONTAINS_ITERS as f64;
    assert_eq!(hits, array_hits);

    // intersection
    let intersect_iters = (2_000_000 / cardinality.max(1)).clamp(10, 5_000);
    let mut roaring_total = 0u64;
    let start = Instant::now();
    for _ in 0..intersect_iters {
        let inter = &roaring_a & &roaring_b;
        roaring_total += inter.len();
    }
    let roaring_intersect_ns = start.elapsed().as_nanos() as f64 / intersect_iters as f64;

    let mut array_total = 0u64;
    let start = Instant::now();
    for _ in 0..intersect_iters {
        let (mut i, mut j) = (0, 0);
        let mut count = 0u64;
        while i < a.len() && j < b.len() {
            match a[i].cmp(&b[j]) {
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
    let array_intersect_ns = start.elapsed().as_nanos() as f64 / intersect_iters as f64;
    assert_eq!(roaring_total, array_total);

    let contains_winner = if roaring_contains_ns < array_contains_ns { "roaring" } else { "array" };
    let intersect_winner = if roaring_intersect_ns < array_intersect_ns { "roaring" } else { "array" };

    println!(
        "cardinality={cardinality:>6}  contains: roaring={roaring_contains_ns:>6.2}ns array={array_contains_ns:>6.2}ns [{contains_winner:>7}]  \
intersect: roaring={roaring_intersect_ns:>8.2}ns array={array_intersect_ns:>8.2}ns [{intersect_winner:>7}]"
    );
}

fn main() {
    let args: Vec<usize> = env::args().skip(1).map(|s| s.parse().expect("cardinality must be an integer")).collect();
    let cardinalities = if args.is_empty() {
        vec![10, 50, 100, 250, 500, 1000, 2000, 4000, 4096, 8000, 16000, 32000]
    } else {
        args
    };

    for c in cardinalities {
        bench_one(c);
    }
}
