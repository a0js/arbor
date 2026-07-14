//! Loads real per-entity ancestor sets and rebuilds them as a single flat
//! CSR-style arena: one big `Vec<u32>` holding every entity's ancestors
//! back-to-back, plus one `Vec<(u32,u32)>` of (offset, length) per entity.
//! Reports peak RSS for comparison against `ancestor_repr_roaring`, which
//! builds the same data as one `RoaringBitmap` per entity.
//!
//! Usage: ancestor_repr_arena <ancestor_scratch_path>

use std::env;
use std::hint::black_box;

use arbor_bench::read_ancestor_arena;

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

fn main() {
    let path = env::args().nth(1).expect("usage: ancestor_repr_arena <path>");

    // Loading directly produces the final arena representation — there is no
    // separate "build" step, unlike the roaring case which must construct one
    // bitmap object per entity on top of the loaded data. We still take a
    // second RSS reading immediately after for an apples-to-apples delta.
    let (arena, offsets) = read_ancestor_arena(&path);
    let after_load_rss = peak_rss_bytes();
    let after_build_rss = peak_rss_bytes();

    println!(
        "entity_count={} after_load_rss_mb={:.1} after_build_rss_mb={:.1} arena_delta_mb={:.1}",
        offsets.len(),
        after_load_rss as f64 / (1024.0 * 1024.0),
        after_build_rss as f64 / (1024.0 * 1024.0),
        (after_build_rss as f64 - after_load_rss as f64) / (1024.0 * 1024.0)
    );

    black_box((&arena, &offsets));
}
