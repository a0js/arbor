//! Loads a packaged snapshot file exactly the way the production authorizer
//! does (`AuthorizerEngine::load`) and reports peak RSS + load time.
//!
//! This isolates the authorizer's runtime memory cost from the indexer's
//! build-time cost measured by `capacity`/`dump_snapshot` — the two are very
//! different numbers (see memory_breakdown for why).
//!
//! Usage: load_snapshot <path>

use std::env;
use std::path::Path;
use std::time::Instant;

use arbor_authorizer::engine::AuthorizerEngine;

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
    let path = env::args().nth(1).expect("usage: load_snapshot <path>");

    let start = Instant::now();
    let engine = AuthorizerEngine::load(Path::new(&path)).expect("load snapshot");
    let load_ms = start.elapsed().as_millis();

    let rss_bytes = peak_rss_bytes();

    println!(
        "path={path} load_ms={load_ms} rss_bytes={rss_bytes} rss_mb={:.1}",
        rss_bytes as f64 / (1024.0 * 1024.0)
    );

    std::hint::black_box(&engine);
}
