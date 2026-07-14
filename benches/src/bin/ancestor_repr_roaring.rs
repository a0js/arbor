//! Loads real per-entity ancestor sets and rebuilds them as `Vec<RoaringBitmap>`
//! — one independently-heap-allocated bitmap per entity, mirroring the current
//! `IndexedEntity::ancestors` design. Reports peak RSS for comparison against
//! `ancestor_repr_arena`, which builds the same data as a single flat arena.
//!
//! Usage: ancestor_repr_roaring <ancestor_scratch_path>

use std::env;
use std::hint::black_box;

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

fn main() {
    let path = env::args().nth(1).expect("usage: ancestor_repr_roaring <path>");

    let (arena, offsets) = read_ancestor_arena(&path);
    let after_load_rss = peak_rss_bytes();

    let bitmaps: Vec<RoaringBitmap> = offsets
        .iter()
        .map(|&(start, len)| {
            let slice = &arena[start as usize..start as usize + len as usize];
            RoaringBitmap::from_sorted_iter(slice.iter().copied()).expect("sorted input")
        })
        .collect();

    let after_build_rss = peak_rss_bytes();

    println!(
        "entity_count={} after_load_rss_mb={:.1} after_build_rss_mb={:.1} roaring_delta_mb={:.1}",
        offsets.len(),
        after_load_rss as f64 / (1024.0 * 1024.0),
        after_build_rss as f64 / (1024.0 * 1024.0),
        (after_build_rss as f64 - after_load_rss as f64) / (1024.0 * 1024.0)
    );

    black_box(&bitmaps);
}
