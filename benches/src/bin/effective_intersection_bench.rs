//! Compares RoaringBitmap `&` intersection against a manual sorted-array
//! merge-intersection for `effective_principal_policies`/`effective_resource_policies`
//! — the fields that actually sit on `check()`'s hot path
//! (`engine.rs`: `let mut eff = p & r; eff &= action_policies;`), unlike
//! `ancestors` which only does point membership.
//!
//! Pairs each entity's principal-side set with its resource-side set by the
//! same index — not a real principal/resource pair, but preserves the real
//! cardinality distribution, which is what matters for a raw operation-speed
//! comparison.
//!
//! Usage: effective_intersection_bench <principal_path> <resource_path>

use std::env;
use std::hint::black_box;
use std::time::Instant;

use arbor_bench::read_ancestor_arena;
use roaring::RoaringBitmap;

fn peak_rss_bytes() -> u64 {
    unsafe {
        let mut usage: libc::rusage = std::mem::zeroed();
        libc::getrusage(libc::RUSAGE_SELF, &mut usage);
        #[cfg(target_os = "macos")]
        {
            usage.ru_maxrss as u64
        }
        #[cfg(not(target_os = "macos"))]
        {
            (usage.ru_maxrss as u64) * 1024
        }
    }
}

/// Two-pointer merge intersection over sorted slices; returns cardinality.
fn array_intersection_count(a: &[u32], b: &[u32]) -> usize {
    let (mut i, mut j) = (0, 0);
    let mut count = 0;
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
    count
}

fn main() {
    let mut args = env::args().skip(1);
    let principal_path = args.next().expect("usage: effective_intersection_bench <principal_path> <resource_path>");
    let resource_path = args.next().expect("missing resource_path");

    let (p_arena, p_offsets) = read_ancestor_arena(&principal_path);
    let (r_arena, r_offsets) = read_ancestor_arena(&resource_path);
    let after_load_rss = peak_rss_bytes();

    assert_eq!(p_offsets.len(), r_offsets.len(), "mismatched entity counts");

    // Only entities that are actually populated on each side are real
    // principals/resources (e.g. users vs files) — pairing by raw entity
    // index mostly intersects a real set against an empty one, which is a
    // cheap degenerate case, not what check() actually exercises.
    let principal_indices: Vec<usize> = p_offsets
        .iter()
        .enumerate()
        .filter(|(_, &(_, l))| l > 0)
        .map(|(i, _)| i)
        .collect();
    let resource_indices: Vec<usize> = r_offsets
        .iter()
        .enumerate()
        .filter(|(_, &(_, l))| l > 0)
        .map(|(i, _)| i)
        .collect();
    let n = principal_indices.len().min(resource_indices.len());
    println!(
        "pairing {n} real (non-empty principal-set, non-empty resource-set) pairs \
(principal pool={}, resource pool={})",
        principal_indices.len(),
        resource_indices.len()
    );

    // --- Memory: RoaringBitmap-per-entity vs arena, for both fields ---
    let p_bitmaps: Vec<RoaringBitmap> = p_offsets
        .iter()
        .map(|&(s, l)| RoaringBitmap::from_sorted_iter(p_arena[s as usize..s as usize + l as usize].iter().copied()).unwrap())
        .collect();
    let r_bitmaps: Vec<RoaringBitmap> = r_offsets
        .iter()
        .map(|&(s, l)| RoaringBitmap::from_sorted_iter(r_arena[s as usize..s as usize + l as usize].iter().copied()).unwrap())
        .collect();
    let after_roaring_build_rss = peak_rss_bytes();

    let roaring_delta_mb = (after_roaring_build_rss as f64 - after_load_rss as f64) / (1024.0 * 1024.0);

    // --- Speed: intersection, same-index pairing ---
    const ITERS: u32 = 5;

    let mut roaring_total: u64 = 0;
    let start = Instant::now();
    for _ in 0..ITERS {
        for k in 0..n {
            let inter = &p_bitmaps[principal_indices[k]] & &r_bitmaps[resource_indices[k]];
            roaring_total += inter.len();
        }
    }
    let roaring_ns = start.elapsed().as_nanos() as f64 / (n as f64 * ITERS as f64);

    let mut array_total: u64 = 0;
    let start = Instant::now();
    for _ in 0..ITERS {
        for k in 0..n {
            let (ps, pl) = p_offsets[principal_indices[k]];
            let (rs, rl) = r_offsets[resource_indices[k]];
            let pa = &p_arena[ps as usize..ps as usize + pl as usize];
            let ra = &r_arena[rs as usize..rs as usize + rl as usize];
            array_total += array_intersection_count(pa, ra) as u64;
        }
    }
    let array_ns = start.elapsed().as_nanos() as f64 / (n as f64 * ITERS as f64);

    println!("n_entities={n}");
    println!(
        "memory: roaring_delta_mb={roaring_delta_mb:.1} (arena baseline already loaded, {after_load_rss}b)"
    );
    println!(
        "speed:  roaring_intersect_ns={roaring_ns:.2} array_merge_intersect_ns={array_ns:.2} \
roaring_total_matches={roaring_total} array_total_matches={array_total}"
    );

    black_box((&p_bitmaps, &r_bitmaps));
}
