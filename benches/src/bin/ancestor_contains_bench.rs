//! Measures `.contains()` cost on the two candidate representations for
//! `ancestors` — the field the arena/CSR conversion targets — using the real
//! per-entity data. `.ancestors` is only ever queried via a single point
//! membership test (`execute_in_hierarchy` in bytecode_vm.rs), never a
//! bitwise AND/OR, so this is the only operation that matters for that field.
//!
//! Usage: ancestor_contains_bench <ancestor_scratch_path>

use std::env;
use std::time::Instant;

use arbor_bench::read_ancestor_arena;
use roaring::RoaringBitmap;

fn main() {
    let path = env::args().nth(1).expect("usage: ancestor_contains_bench <path>");
    let (arena, offsets) = read_ancestor_arena(&path);

    let bitmaps: Vec<RoaringBitmap> = offsets
        .iter()
        .map(|&(start, len)| {
            let slice = &arena[start as usize..start as usize + len as usize];
            RoaringBitmap::from_sorted_iter(slice.iter().copied()).expect("sorted input")
        })
        .collect();

    // Probe each entity's own set with: (a) its last real ancestor (hit) and
    // (b) a value one past its max (guaranteed miss) — exercises both the
    // present and absent cases, same as InHierarchy would in practice.
    const ITERS: u32 = 5;
    let n = offsets.len();

    let mut roaring_hits = 0u64;
    let start = Instant::now();
    for _ in 0..ITERS {
        for (i, &(off, len)) in offsets.iter().enumerate() {
            if len == 0 {
                continue;
            }
            let probe_hit = arena[off as usize + len as usize - 1];
            let probe_miss = arena[off as usize] .wrapping_add(1_000_000_000);
            if bitmaps[i].contains(probe_hit) {
                roaring_hits += 1;
            }
            if bitmaps[i].contains(probe_miss) {
                roaring_hits += 1;
            }
        }
    }
    let roaring_ns = start.elapsed().as_nanos() as f64 / (n as f64 * ITERS as f64 * 2.0);

    let mut array_hits = 0u64;
    let start = Instant::now();
    for _ in 0..ITERS {
        for &(off, len) in &offsets {
            if len == 0 {
                continue;
            }
            let slice = &arena[off as usize..off as usize + len as usize];
            let probe_hit = arena[off as usize + len as usize - 1];
            let probe_miss = arena[off as usize].wrapping_add(1_000_000_000);
            if slice.binary_search(&probe_hit).is_ok() {
                array_hits += 1;
            }
            if slice.binary_search(&probe_miss).is_ok() {
                array_hits += 1;
            }
        }
    }
    let array_ns = start.elapsed().as_nanos() as f64 / (n as f64 * ITERS as f64 * 2.0);

    println!(
        "n_entities={n} roaring_contains_ns={roaring_ns:.2} array_binary_search_ns={array_ns:.2} \
roaring_hits={roaring_hits} array_hits={array_hits}"
    );
}
