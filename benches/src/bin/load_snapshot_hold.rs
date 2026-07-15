//! Same load path as `load_snapshot`, but parks the process afterward
//! instead of exiting, so external tools (`vmmap`, `heap`, `leaks`) can
//! attach and inspect *actual resident memory in steady state* -- as
//! opposed to `getrusage`'s peak-RSS high-water mark, which `load_snapshot`
//! reports and which includes transient decode-time allocations that are
//! already freed by the time the process would otherwise exit.
//!
//! Usage: load_snapshot_hold <path> [hold_secs]
//!   Prints its own PID immediately after loading, then sleeps so you can
//!   run `vmmap <pid>` / `heap <pid>` from another shell before it exits.

use std::env;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use arbor_authorizer::engine::AuthorizerEngine;

fn main() {
    let mut args = env::args().skip(1);
    let path = args.next().expect("usage: load_snapshot_hold <path> [hold_secs]");
    let hold_secs: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(60);

    let start = Instant::now();
    let engine = AuthorizerEngine::load(Path::new(&path)).expect("load snapshot");
    let load_ms = start.elapsed().as_millis();

    // Force a full GC-equivalent: drop nothing here (Rust has no GC), but
    // give the allocator a moment to settle/trim before we report the PID.
    println!("pid={} load_ms={load_ms} holding for {hold_secs}s -- attach now", std::process::id());
    std::hint::black_box(&engine);

    thread::sleep(Duration::from_secs(hold_secs));

    std::hint::black_box(&engine);
}
