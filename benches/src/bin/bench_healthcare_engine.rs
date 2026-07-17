//! Benchmarks `AuthorizerEngine::check()` and `list_entities()` against the
//! real healthcare test dataset (`data/healthcare`), broken down by *which
//! kind* of policy match actually decides each call/list -- plain type-wide
//! RBAC, `EntityWithDescendants` ReBAC through the multi-parent
//! specialty-group DAG, an ABAC condition that actually evaluates (not
//! short-circuited), and the entity with the largest effective-policy set
//! in the whole dataset (found by scanning, not guessed) -- rather than one
//! fixed scenario like `capacity.rs`'s synthetic `build_scenario`.
//!
//! The question this answers: does a conditional policy, a policy-dense
//! entity, or a broad (`EntityType`) candidate set cost meaningfully more
//! than the flat ~165-176ns `check()` baseline the capacity-benchmarks
//! branch measured on synthetic data?
//!
//! Unlike `capacity.rs`'s batched timing (one `Instant` around N iterations,
//! divided down -- the lowest-overhead way to get a clean average), this
//! tool times each call individually to get percentiles, which means every
//! number here carries a constant per-call `Instant::now()` overhead on top
//! of the real cost. That overhead is identical across every case, so
//! relative comparisons between them are still meaningful; treat the
//! *absolute* ns figures as an upper bound, not the canonical cost -- for
//! that, `capacity.rs`/`crossover_sweep.rs` remain the reference.
//!
//! Usage: bench_healthcare_engine [--config-dir DIR] [--check-iters N] [--list-iters N]

use std::collections::{HashMap, HashSet};
use std::env;
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::time::Instant;

use arbor_authorizer::engine::AuthorizerEngine;
use arbor_connectors::action_id_for_name;
use arbor_index_snapshot::PolicySide;
use arbor_indexer::{csv_source, snapshot_builder::SnapshotBuilder};
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

fn read_rows(dir: &Path, file: &str) -> Vec<HashMap<String, String>> {
    let path = dir.join(file);
    let mut reader = csv::Reader::from_path(&path)
        .unwrap_or_else(|e| panic!("failed to open {}: {e}", path.display()));
    let headers = reader.headers().expect("headers").clone();
    reader
        .records()
        .map(|r| {
            let record = r.expect("valid csv row");
            headers.iter().zip(record.iter()).map(|(h, v)| (h.to_string(), v.to_string())).collect()
        })
        .collect()
}

fn parents(row: &HashMap<String, String>) -> Vec<Uuid> {
    row["parent_ids"]
        .split(';')
        .filter(|s| !s.is_empty())
        .map(|s| Uuid::parse_str(s).unwrap_or_else(|e| panic!("bad uuid {s:?}: {e}")))
        .collect()
}

fn uuid(row: &HashMap<String, String>, col: &str) -> Uuid {
    Uuid::parse_str(&row[col]).unwrap_or_else(|e| panic!("bad uuid in column {col:?}: {e}"))
}

fn percentile(sorted: &[u64], p: f64) -> u64 {
    let idx = (((sorted.len() - 1) as f64) * p).round() as usize;
    sorted[idx]
}

/// Times `call` `iters` times (plus one untimed warmup), sorts the samples,
/// and prints one summary row: `name`, then "how much work did this case
/// actually do" (the u64 `call` returns -- effective-policy-set size for
/// `check()` cases, result-entity count for `list_entities()` cases, taken
/// from the warmup call since a deterministic query returns the same size
/// every time), then avg/p50/p99/max latency.
fn bench_row(name: &str, iters: u32, mut call: impl FnMut() -> u64) {
    let work = call(); // warmup, and the work-size reading
    let mut samples = Vec::with_capacity(iters as usize);
    for _ in 0..iters {
        let start = Instant::now();
        let result = call();
        let elapsed = start.elapsed().as_nanos() as u64;
        black_box(result);
        samples.push(elapsed);
    }
    samples.sort_unstable();
    let avg = samples.iter().sum::<u64>() as f64 / samples.len() as f64;
    println!(
        "{:<58} {:>10} {:>9.0} {:>9} {:>9} {:>9}",
        name,
        work,
        avg,
        percentile(&samples, 0.5),
        percentile(&samples, 0.99),
        samples[samples.len() - 1],
    );
}

fn table_header(work_col: &str) {
    println!("{:<58} {:>10} {:>9} {:>9} {:>9} {:>9}", "case", work_col, "avg_ns", "p50_ns", "p99_ns", "max_ns");
}

struct CheckCase {
    name: &'static str,
    principal: u32,
    action: u32,
    resource: u32,
}

struct ListCase {
    name: &'static str,
    fixed: u32,
    action: u32,
    candidate_type_name: &'static str,
    side: PolicySide,
}

fn describe_resource(engine: &AuthorizerEngine, resource_idx: u32) -> (usize, usize) {
    let effective = engine.snapshot().effective_resource_of(resource_idx);
    let conditional = effective
        .iter()
        .filter(|&&idx| engine.snapshot().get_policy(idx).is_some_and(|p| p.is_conditional))
        .count();
    (effective.len(), conditional)
}

fn main() {
    let mut config_dir = PathBuf::from("data/healthcare");
    let mut check_iters: u32 = 20_000;
    let mut list_iters: u32 = 200;
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config-dir" => config_dir = PathBuf::from(args.next().expect("--config-dir needs a value")),
            "--check-iters" => check_iters = args.next().expect("--check-iters needs a value").parse().expect("must be a positive integer"),
            "--list-iters" => list_iters = args.next().expect("--list-iters needs a value").parse().expect("must be a positive integer"),
            other => panic!("unrecognized argument: {other}"),
        }
    }
    if !config_dir.exists() {
        panic!(
            "{} does not exist -- generate it first with:\n  cargo run -p arbor-bench --bin gen_healthcare_dataset -- --out-dir {}",
            config_dir.display(),
            config_dir.display(),
        );
    }

    println!("Loading graph from connectors in {}", config_dir.display());
    let connectors = arbor_connectors::load_connector_config(&config_dir).expect("load connectors.yaml");
    let data_model = arbor_connectors::load_data_model_config(&config_dir).expect("load data_model.yaml");
    let graph = csv_source::build_graph(&connectors, &data_model, &config_dir).expect("build graph from connectors");

    let build_start = Instant::now();
    let snapshot = SnapshotBuilder::build(&graph).expect("build snapshot");
    let build_ms = build_start.elapsed().as_millis();
    let node_count = snapshot.nodes.len();

    let engine = AuthorizerEngine::from_snapshot(snapshot);
    let rss_mb = peak_rss_bytes() as f64 / (1024.0 * 1024.0);
    println!("node_count={node_count} build_ms={build_ms} rss_mb={rss_mb:.1}\n");

    let read_chart = action_id_for_name("read_chart");
    let read_labs = action_id_for_name("read_labs");
    let read_billing = action_id_for_name("read_billing");
    let room_read = action_id_for_name("read");
    let idx_of = |id: Uuid| engine.snapshot().uuid_to_index(&id).unwrap_or_else(|| panic!("uuid {id} not found in snapshot"));
    let type_id = |name: &str| engine.snapshot().get_entity_type_id_by_name(name).unwrap_or_else(|| panic!("entity type {name:?} not found"));

    let physicians = read_rows(&config_dir, "physicians.csv");
    let patients = read_rows(&config_dir, "patients.csv");
    let admin_staff = read_rows(&config_dir, "admin_staff.csv");
    let technicians = read_rows(&config_dir, "technicians.csv");
    let rooms = read_rows(&config_dir, "rooms_equipment.csv");
    let policies = read_rows(&config_dir, "policies.csv");

    let mut roster_of_clinic: HashMap<Uuid, Uuid> = HashMap::new();
    for row in &policies {
        if row["policy_name"].ends_with("-staff-read-write-own-roster") {
            roster_of_clinic.insert(uuid(row, "principal"), uuid(row, "resource"));
        }
    }

    let restricted_patients: HashSet<Uuid> = policies
        .iter()
        .filter(|row| row["ptype"] == "forbid" && row["principal_kind"] == "entity_type" && row["principal"] == "Physician")
        .map(|row| uuid(row, "resource"))
        .collect();

    let physician_row = &physicians[0];
    let physician_idx = idx_of(uuid(physician_row, "physician_id"));
    let phys_parents = parents(physician_row);
    let physician_clinic = phys_parents[0];
    let physician_group_idx = idx_of(phys_parents[1]);
    let physician_roster = roster_of_clinic[&physician_clinic];

    let patient_roster = |row: &HashMap<String, String>| parents(row)[0];
    let not_restricted = |row: &&HashMap<String, String>| !restricted_patients.contains(&uuid(row, "patient_id"));

    let baseline_patient_idx = idx_of(uuid(
        patients.iter().filter(not_restricted).find(|p| patient_roster(p) == physician_roster).expect("a plain patient"),
        "patient_id",
    ));
    let conditional_permit_idx = idx_of(uuid(
        patients
            .iter()
            .filter(not_restricted)
            .find(|p| patient_roster(p) != physician_roster && p["consents_to_specialist_sharing"] == "true")
            .expect("a consenting cross-clinic patient"),
        "patient_id",
    ));
    let conditional_deny_idx = idx_of(uuid(
        patients
            .iter()
            .filter(not_restricted)
            .find(|p| patient_roster(p) != physician_roster && p["consents_to_specialist_sharing"] == "false")
            .expect("a non-consenting cross-clinic patient"),
        "patient_id",
    ));

    let (max_policy_patient_idx, _) = patients
        .iter()
        .map(|p| idx_of(uuid(p, "patient_id")))
        .map(|idx| (idx, describe_resource(&engine, idx).0))
        .max_by_key(|&(_, effective)| effective)
        .expect("at least one patient");

    let admin_idx = idx_of(uuid(&admin_staff[0], "admin_id"));
    let technician_idx = idx_of(uuid(&technicians[0], "tech_id"));
    let room_idx = idx_of(uuid(&rooms[0], "room_id"));

    let referral_row = policies.iter().find(|row| row["policy_name"].starts_with("referral-")).expect("at least one referral");
    let referral_physician_idx = idx_of(uuid(referral_row, "principal"));
    let referral_patient_idx = idx_of(uuid(referral_row, "resource"));

    let restricted_patient_idx = idx_of(*restricted_patients.iter().next().expect("at least one restricted patient"));

    assert_ne!(physician_group_idx, physician_idx, "sanity: group and clinic must be distinct entities");

    // --- check() ---

    let check_cases = vec![
        CheckCase {
            name: "baseline: same-clinic RBAC, unconditional",
            principal: physician_idx,
            action: idx_of(read_chart),
            resource: baseline_patient_idx,
        },
        CheckCase {
            name: "ReBAC via multi-parent group, condition evaluates TRUE",
            principal: physician_idx,
            action: idx_of(read_chart),
            resource: conditional_permit_idx,
        },
        CheckCase {
            name: "ReBAC via multi-parent group, condition evaluates FALSE",
            principal: physician_idx,
            action: idx_of(read_chart),
            resource: conditional_deny_idx,
        },
        CheckCase {
            name: "most policy-dense patient in the dataset",
            principal: physician_idx,
            action: idx_of(read_chart),
            resource: max_policy_patient_idx,
        },
        CheckCase {
            name: "referral: narrow Entity->Entity ActionSet grant",
            principal: referral_physician_idx,
            action: idx_of(read_labs),
            resource: referral_patient_idx,
        },
        CheckCase {
            name: "admin billing: EntityType->EntityType, unconditional",
            principal: admin_idx,
            action: idx_of(read_billing),
            resource: baseline_patient_idx,
        },
        CheckCase {
            name: "technician room access: EntityType->EntityType",
            principal: technician_idx,
            action: idx_of(room_read),
            resource: room_idx,
        },
    ];

    println!("check() -- work column is the resource's effective-policy-set size:");
    table_header("eff_pols");
    for case in &check_cases {
        let eff_pols = describe_resource(&engine, case.resource).0 as u64;
        bench_row(case.name, check_iters, || {
            engine.check(case.principal, case.action, case.resource).expect("check failed");
            eff_pols
        });
    }

    // --- list_entities() ---

    let patient_type = type_id("Patient");
    let physician_type = type_id("Physician");
    let room_type = type_id("RoomEquipment");

    let list_cases = vec![
        ListCase {
            name: "list_resources: patients a physician can read_chart (broad ReBAC + ABAC)",
            fixed: physician_idx,
            action: idx_of(read_chart),
            candidate_type_name: "Patient",
            side: PolicySide::Resource,
        },
        ListCase {
            name: "list_resources: rooms a technician can access (narrow, unconditional)",
            fixed: technician_idx,
            action: idx_of(room_read),
            candidate_type_name: "RoomEquipment",
            side: PolicySide::Resource,
        },
        ListCase {
            name: "list_resources: patients admin can read_billing (broad, unconditional)",
            fixed: admin_idx,
            action: idx_of(read_billing),
            candidate_type_name: "Patient",
            side: PolicySide::Resource,
        },
        ListCase {
            name: "list_principals: physicians who can read a plain patient's chart",
            fixed: baseline_patient_idx,
            action: idx_of(read_chart),
            candidate_type_name: "Physician",
            side: PolicySide::Principal,
        },
        ListCase {
            name: "list_principals: physicians who can read a RESTRICTED patient's chart",
            fixed: restricted_patient_idx,
            action: idx_of(read_chart),
            candidate_type_name: "Physician",
            side: PolicySide::Principal,
        },
    ];

    println!("\nlist_entities() -- work column is the number of entities returned:");
    table_header("n_result");
    for case in &list_cases {
        let candidate_type = match case.candidate_type_name {
            "Patient" => patient_type,
            "Physician" => physician_type,
            "RoomEquipment" => room_type,
            other => panic!("unmapped candidate type {other:?}"),
        };
        bench_row(case.name, list_iters, || {
            let result = engine.list_entities(case.fixed, case.action, candidate_type, case.side).expect("list_entities failed");
            result.indices.len() as u64
        });
    }
}
