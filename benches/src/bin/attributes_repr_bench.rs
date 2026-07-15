//! Measures the real per-entity cost of `Attributes` (a `BTreeMap` per
//! entity) against a flattened alternative, using the same "build N real
//! objects, measure RSS delta" technique used throughout this session for
//! the RoaringBitmap fields -- `Attributes` was never touched by that work
//! and may be the dominant remaining per-entity allocation cost (the
//! tracked CSR arenas + policy bitmaps only account for ~173MB out of a
//! measured 1060MB snapshot RSS at 1M entities).
//!
//! Usage: attributes_repr_bench <n_entities> <attrs_per_entity>

use std::env;

use arbor_types::{AttributeNameId, AttributeValue, Attributes};

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
    let mut args = env::args().skip(1);
    let n: usize = args
        .next()
        .expect("usage: attributes_repr_bench <n_entities> <attrs_per_entity>")
        .parse()
        .expect("n_entities must be a positive integer");
    let attrs_per_entity: usize = args
        .next()
        .unwrap_or_else(|| "2".to_string())
        .parse()
        .expect("attrs_per_entity must be a positive integer");

    let before_rss = peak_rss_bytes();

    // Current design: one Attributes (BTreeMap) per entity.
    let mut all_attrs: Vec<Attributes> = Vec::with_capacity(n);
    for i in 0..n {
        let mut a = Attributes::new();
        for k in 0..attrs_per_entity {
            a.set(
                AttributeNameId::new(k as u32),
                AttributeValue::Integer((i % 5) as i64),
            );
        }
        all_attrs.push(a);
    }

    let after_rss = peak_rss_bytes();
    let delta_mb = (after_rss as f64 - before_rss as f64) / (1024.0 * 1024.0);

    println!(
        "n_entities={n} attrs_per_entity={attrs_per_entity} btreemap_delta_mb={delta_mb:.1} \
bytes_per_entity={:.1}",
        (after_rss - before_rss) as f64 / n as f64
    );

    // Flattened alternative: one shared arena of (AttributeNameId, AttributeValue)
    // pairs + (offset, len) per entity -- same CSR pattern as the RoaringBitmap work.
    let before_rss2 = peak_rss_bytes();
    let mut arena: Vec<(AttributeNameId, AttributeValue)> = Vec::with_capacity(n * attrs_per_entity);
    let mut offsets: Vec<(u32, u32)> = Vec::with_capacity(n);
    for i in 0..n {
        let start = arena.len() as u32;
        for k in 0..attrs_per_entity {
            arena.push((AttributeNameId::new(k as u32), AttributeValue::Integer((i % 5) as i64)));
        }
        offsets.push((start, attrs_per_entity as u32));
    }
    let after_rss2 = peak_rss_bytes();
    let delta_mb2 = (after_rss2 as f64 - before_rss2 as f64) / (1024.0 * 1024.0);

    println!(
        "flattened arena: delta_mb={delta_mb2:.1} bytes_per_entity={:.1}",
        (after_rss2 - before_rss2) as f64 / n as f64
    );

    std::hint::black_box((&all_attrs, &arena, &offsets));
}
