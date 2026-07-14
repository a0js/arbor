//! Single-scale memory/build-time probe.
//!
//! Builds one [`arbor_bench::build_scenario`] graph + snapshot for the given
//! entity count and reports peak resident set size. Intentionally a plain
//! binary rather than a criterion bench: each invocation is a fresh process,
//! so a scale large enough to OOM only kills that one process instead of
//! corrupting the measurements for every other scale in the sweep (see
//! `scripts/capacity_sweep.sh`).
//!
//! Usage: capacity <n_entities>

use std::env;
use std::hint::black_box;
use std::time::Instant;

use arbor_authorizer::engine::AuthorizerEngine;
use arbor_bench::build_scenario;
use arbor_index_snapshot::PolicySide;

const CHECK_ITERS: u32 = 2_000;
const LIST_ITERS: u32 = 50;

/// Average wall-clock time per call, in nanoseconds, over `iters` calls.
fn avg_ns(iters: u32, mut call: impl FnMut()) -> f64 {
    let start = Instant::now();
    for _ in 0..iters {
        call();
    }
    start.elapsed().as_nanos() as f64 / iters as f64
}

fn peak_rss_bytes() -> u64 {
    unsafe {
        let mut usage: libc::rusage = std::mem::zeroed();
        libc::getrusage(libc::RUSAGE_SELF, &mut usage);
        // macOS reports ru_maxrss in bytes; Linux reports it in kilobytes.
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
    let n: usize = env::args()
        .nth(1)
        .expect("usage: capacity <n_entities>")
        .parse()
        .expect("n_entities must be a positive integer");

    let start = Instant::now();
    let (snapshot, fixtures) = build_scenario(n);
    let build_ms = start.elapsed().as_millis();

    let node_count = snapshot.nodes.len();
    let engine = AuthorizerEngine::from_snapshot(snapshot);

    // Warm up (fill any lazily-populated caches) before timing.
    engine
        .check(fixtures.permitted_principal, fixtures.action, fixtures.resource)
        .expect("warmup check failed");

    let check_permitted_ns = avg_ns(CHECK_ITERS, || {
        black_box(
            engine
                .check(fixtures.permitted_principal, fixtures.action, fixtures.resource)
                .expect("check failed"),
        );
    });
    let check_denied_ns = avg_ns(CHECK_ITERS, || {
        black_box(
            engine
                .check(fixtures.denied_principal, fixtures.action, fixtures.resource)
                .expect("check failed"),
        );
    });
    let list_resources_ns = avg_ns(LIST_ITERS, || {
        black_box(
            engine
                .list_entities(
                    fixtures.permitted_principal,
                    fixtures.action,
                    fixtures.file_type,
                    PolicySide::Resource,
                )
                .expect("list_entities failed"),
        );
    });
    let list_principals_ns = avg_ns(LIST_ITERS, || {
        black_box(
            engine
                .list_entities(
                    fixtures.resource,
                    fixtures.action,
                    fixtures.file_type,
                    PolicySide::Principal,
                )
                .expect("list_entities failed"),
        );
    });

    // Peak RSS is monotonic for this workload, so sampling last still
    // captures the high-water mark from building the graph/snapshot.
    let rss_bytes = peak_rss_bytes();

    println!(
        "n_entities={n} node_count={node_count} build_ms={build_ms} \
rss_bytes={rss_bytes} rss_mb={:.1} \
check_permitted_ns={check_permitted_ns:.0} check_denied_ns={check_denied_ns:.0} \
list_resources_ns={list_resources_ns:.0} list_principals_ns={list_principals_ns:.0}",
        rss_bytes as f64 / (1024.0 * 1024.0)
    );
}
