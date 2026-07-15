//! Loads the archive written by `rkyv_dump_snapshot` the zero-copy way
//! (`rkyv::access`: validate + cast, no per-element allocation) and reports
//! peak RSS, same methodology as `load_snapshot` (separate process, same
//! `getrusage` measurement) so the two are directly comparable.
//!
//! Usage: rkyv_load_snapshot <path>

use std::env;
use std::fs;
use std::time::Instant;

use rkyv::rancor::Error;
use rkyv::{Archive, Deserialize, Serialize};

#[derive(Archive, Serialize, Deserialize)]
struct ProtoPolicy {
    idx: u32,
    principal_target: u32,
    resource_target: u32,
    is_forbidding: bool,
    is_conditional: bool,
}

#[derive(Archive, Serialize, Deserialize)]
struct ProtoSnapshot {
    ancestors_arena: Vec<u32>,
    principal_of_arena: Vec<u32>,
    resource_of_arena: Vec<u32>,
    effective_principal_arena: Vec<u32>,
    effective_resource_arena: Vec<u32>,
    attribute_strings: Vec<String>,
    policies: Vec<ProtoPolicy>,
}

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
    let path = env::args().nth(1).expect("usage: rkyv_load_snapshot <path>");

    let start = Instant::now();
    let bytes = fs::read(&path).expect("read archive file");
    let archived =
        rkyv::access::<ArchivedProtoSnapshot, Error>(&bytes).expect("rkyv access/validate");
    let load_ms = start.elapsed().as_millis();

    let rss_bytes = peak_rss_bytes();

    println!(
        "path={path} load_ms={load_ms} ancestors_len={} attribute_strings_len={} policies_len={} \
rss_bytes={rss_bytes} rss_mb={:.1}",
        archived.ancestors_arena.len(),
        archived.attribute_strings.len(),
        archived.policies.len(),
        rss_bytes as f64 / (1024.0 * 1024.0)
    );

    std::hint::black_box(&archived);
}
