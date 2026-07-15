//! Builds a *synthetic* snapshot-shaped structure sized to match the real
//! 1M-entity `Snapshot`'s measured dominant contributors exactly (arena
//! lengths from `check_vec_capacity`, attribute-value string count/avg size
//! and policy count from the `malloc_history` allocation-category
//! breakdown), archives it with `rkyv`, and writes the archive to disk.
//!
//! This is NOT a port of the real `Snapshot` -- `Uuid`, `RoaringBitmap`,
//! `chrono`, and `ipnet` would each need their own `rkyv` trait impls or
//! wrappers, which is the real migration, not a prototype. This measures
//! the two things that generalize regardless of those details: archive
//! size vs bincode, and (via `rkyv_load_snapshot`) load-time peak RSS vs
//! bincode+lz4's allocate-per-node deserialize.
//!
//! Sizes match the real 1M-entity snapshot measured earlier in this branch:
//!   ancestors_arena             9,684,479 u32
//!   principal_of_arena             13,331 u32
//!   resource_of_arena              13,331 u32
//!   effective_principal_arena   3,156,705 u32
//!   effective_resource_arena    8,096,183 u32
//!   attribute value strings       500,002 @ ~104 bytes avg (malloc_history)
//!   policies                       13,336
//!
//! Usage: rkyv_dump_snapshot <output_path>

use std::env;
use std::fs;

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

fn arena(n: usize) -> Vec<u32> {
    (0..n as u32).collect()
}

fn main() {
    let output_path = env::args().nth(1).expect("usage: rkyv_dump_snapshot <output_path>");

    let attribute_strings: Vec<String> = (0..500_002)
        .map(|i| {
            // ~104 bytes average, matching the real attribute-value strings'
            // measured average size from malloc_history (49.6MB / 500,002).
            format!("attr-value-{i:0>92}")
        })
        .collect();

    let policies: Vec<ProtoPolicy> = (0..13_336)
        .map(|i| ProtoPolicy {
            idx: i,
            principal_target: i,
            resource_target: i,
            is_forbidding: i % 7 == 0,
            is_conditional: i % 5 == 0,
        })
        .collect();

    let snapshot = ProtoSnapshot {
        ancestors_arena: arena(9_684_479),
        principal_of_arena: arena(13_331),
        resource_of_arena: arena(13_331),
        effective_principal_arena: arena(3_156_705),
        effective_resource_arena: arena(8_096_183),
        attribute_strings,
        policies,
    };

    let bytes = rkyv::to_bytes::<Error>(&snapshot).expect("rkyv serialize");

    fs::write(&output_path, &bytes).expect("write archive file");

    println!(
        "output={output_path} archive_bytes={} archive_mb={:.1}",
        bytes.len(),
        bytes.len() as f64 / (1024.0 * 1024.0)
    );
}
