//! Times check() on an engine loaded from a packaged snapshot file (the
//! actual `AuthorizerEngine::load` path, same as production and same as
//! `load_snapshot`) -- as opposed to `capacity`, which times check() on an
//! engine built in-process via `from_snapshot`, never touching the
//! deserialize path at all. Exists to answer: does reserving exact Vec
//! capacity on deserialize (`reserved_vec_serde`) change check() latency,
//! since the resulting arenas/nodes Vec now have cap==len instead of
//! cap>=len from the old doubling growth?
//!
//! Fixtures aren't stored in the packaged file (`dump_snapshot` discards
//! them), so this re-derives them via `build_scenario(n)`, which is
//! deterministic -- same n always yields the same fixture indices.
//!
//! Usage: check_after_load <n_entities> <path>

use std::env;
use std::hint::black_box;
use std::path::Path;
use std::time::Instant;

use arbor_authorizer::engine::AuthorizerEngine;
use arbor_bench::build_scenario;

const CHECK_ITERS: u32 = 2_000;

fn avg_ns(iters: u32, mut call: impl FnMut()) -> f64 {
    let start = Instant::now();
    for _ in 0..iters {
        call();
    }
    start.elapsed().as_nanos() as f64 / iters as f64
}

fn main() {
    let mut args = env::args().skip(1);
    let n: usize = args.next().expect("usage: check_after_load <n_entities> <path>").parse().expect("n_entities must be a positive integer");
    let path = args.next().expect("usage: check_after_load <n_entities> <path>");

    // Only need the fixtures here; the Snapshot this produces is discarded.
    let (_discarded_snapshot, fixtures) = build_scenario(n);

    let start = Instant::now();
    let engine = AuthorizerEngine::load(Path::new(&path)).expect("load snapshot");
    let load_ms = start.elapsed().as_millis();

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
                .check(fixtures.denied_principal, fixtures.delete_action, fixtures.resource)
                .expect("check failed"),
        );
    });

    println!(
        "n_entities={n} path={path} load_ms={load_ms} check_permitted_ns={check_permitted_ns:.0} check_denied_ns={check_denied_ns:.0}"
    );
}
