//! Measures the real RSS cost of building a `RapidHashMap<Uuid, u32>` with
//! 1M entries under different construction strategies, to test whether the
//! custom `rapid_hash_map_serde` deserialization path (insert one at a time,
//! reserve once via `size_hint()`) causes more peak memory than a
//! properly-capacity-reserved bulk build -- and to get a real number for
//! the table's steady-state size independent of insertion method, since
//! `RapidHashMap` was confirmed to be structurally identical to
//! `std::collections::HashMap` (same layout, different hasher only).
//!
//! Runs ONE strategy per process invocation (not all in one process) so the
//! allocator can't reuse freed capacity from a prior strategy and skew the
//! comparison.
//!
//! Usage: rapidhashmap_repr_bench <strategy>
//!   with_capacity  -- HashMap::with_capacity(n) then insert
//!   no_reserve     -- HashMap::new() then insert with no upfront reserve
//!   reserve_hint   -- HashMap::new(), reserve(n) once, then insert
//!                     (mirrors rapid_hash_map_serde's visit_seq exactly)
//!   from_iter      -- collect() from an iterator in one shot

use std::env;

use rapidhash::RapidHashMap;
use uuid::Uuid;

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

const N: usize = 1_000_000;

fn main() {
    let strategy = env::args().nth(1).expect("usage: rapidhashmap_repr_bench <strategy>");

    // Generate the keys first, in an isolated scope, so their own
    // allocation (a Vec<Uuid>) is a separate, known, subtracted baseline.
    let keys: Vec<Uuid> = (0..N).map(|_| Uuid::new_v4()).collect();
    let before_rss = peak_rss_bytes();

    let map: RapidHashMap<Uuid, u32> = match strategy.as_str() {
        "with_capacity" => {
            let mut m = RapidHashMap::with_capacity_and_hasher(N, Default::default());
            for (i, &k) in keys.iter().enumerate() {
                m.insert(k, i as u32);
            }
            m
        }
        "no_reserve" => {
            let mut m: RapidHashMap<Uuid, u32> = RapidHashMap::default();
            for (i, &k) in keys.iter().enumerate() {
                m.insert(k, i as u32);
            }
            m
        }
        "reserve_hint" => {
            // Mirrors rapid_hash_map_serde::deserialize::visit_seq exactly:
            // default-construct, reserve(size_hint) once, then insert in a loop.
            let mut m: RapidHashMap<Uuid, u32> = RapidHashMap::default();
            m.reserve(keys.len());
            for (i, &k) in keys.iter().enumerate() {
                m.insert(k, i as u32);
            }
            m
        }
        "from_iter" => keys.iter().enumerate().map(|(i, &k)| (k, i as u32)).collect(),
        other => panic!("unknown strategy: {other}"),
    };

    let after_rss = peak_rss_bytes();
    let delta_mb = (after_rss as f64 - before_rss as f64) / (1024.0 * 1024.0);

    println!(
        "strategy={strategy} n={N} map_len={} before_rss_mb={:.1} after_rss_mb={:.1} delta_mb={:.1} bytes_per_entry={:.1}",
        map.len(),
        before_rss as f64 / (1024.0 * 1024.0),
        after_rss as f64 / (1024.0 * 1024.0),
        delta_mb,
        (after_rss - before_rss) as f64 / N as f64
    );

    std::hint::black_box(&map);
}
