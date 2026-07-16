//! Real correctness check for `RkyvSnapshot`: builds the same deterministic
//! scenario twice (once kept as a plain `Snapshot` reference, once archived
//! to a file and reloaded via `RkyvSnapshot::load`), then compares every
//! `EntityResolver`/`SnapshotOps` method's output between the two backing
//! stores across a spread of indices -- not just "it compiles", actual
//! behavioral equivalence.
//!
//! Usage: rkyv_snapshot_correctness_check <n_entities>

use std::env;

use arbor_bench::build_scenario;
use arbor_index_snapshot::{PolicySide, RkyvSnapshot, SnapshotOps};
use arbor_types::EntityResolver;
use rkyv::rancor::Error;

fn main() {
    let n: usize = env::args()
        .nth(1)
        .expect("usage: rkyv_snapshot_correctness_check <n_entities>")
        .parse()
        .expect("n_entities must be a positive integer");

    let (reference, fixtures) = build_scenario(n);
    let (archived_source, _) = build_scenario(n); // deterministic: identical to `reference`

    let bytes = rkyv::to_bytes::<Error>(&archived_source).expect("rkyv serialize");
    let compressed = lz4_flex::compress_prepend_size(&bytes);
    let path = std::env::temp_dir().join(format!("rkyv_correctness_{n}.rkyv"));
    std::fs::write(&path, &compressed).expect("write archive");

    let rkyv_snap = RkyvSnapshot::load(&path).expect("RkyvSnapshot::load");

    let mut checked = 0u64;

    // Spread of indices: fixtures (known-interesting) plus a stride across
    // the whole node range to catch anything fixture-only coverage would miss.
    let mut indices: Vec<u32> = vec![fixtures.permitted_principal, fixtures.denied_principal, fixtures.resource];
    let node_count = reference.nodes.len() as u32;
    let mut i = 0u32;
    while i < node_count {
        indices.push(i);
        i += (node_count / 200).max(1);
    }

    for &idx in &indices {
        let ref_entity = reference.get_entity(idx);
        let rkyv_entity = EntityResolver::get_entity(&rkyv_snap, idx);
        assert_eq!(ref_entity.is_some(), rkyv_entity.is_some(), "get_entity presence mismatch at {idx}");
        if let (Some(re), Some(rk)) = (ref_entity, rkyv_entity) {
            assert_eq!(re, rk, "get_entity mismatch at {idx}");
            checked += 1;

            assert_eq!(
                reference.ancestors_of(idx),
                EntityResolver::ancestors_of(&rkyv_snap, idx),
                "ancestors_of mismatch at {idx}"
            );
            assert_eq!(
                SnapshotOps::effective_principal_of(&reference, idx),
                SnapshotOps::effective_principal_of(&rkyv_snap, idx),
                "effective_principal_of mismatch at {idx}"
            );
            assert_eq!(
                SnapshotOps::effective_resource_of(&reference, idx),
                SnapshotOps::effective_resource_of(&rkyv_snap, idx),
                "effective_resource_of mismatch at {idx}"
            );

            // Attribute path resolution, if this entity has any attributes.
            if !re.attributes.is_empty() {
                let pairs_ref = reference.attribute_pairs(re.attributes);
                if let Some((name, _)) = pairs_ref.first() {
                    let ref_view = reference.resolve_attribute_path(re.attributes, std::slice::from_ref(name));
                    let rkyv_view = rkyv_snap.resolve_attribute_path(re.attributes, std::slice::from_ref(name));
                    assert_eq!(format!("{ref_view:?}"), format!("{rkyv_view:?}"), "resolve_attribute_path mismatch at {idx}");
                }
            }
        }

        let ref_policy = reference.get_policy(idx);
        let rkyv_policy = SnapshotOps::get_policy(&rkyv_snap, idx);
        assert_eq!(ref_policy.is_some(), rkyv_policy.is_some(), "get_policy presence mismatch at {idx}");
        if let (Some(rp), Some(kp)) = (ref_policy, rkyv_policy) {
            assert_eq!(rp.idx, kp.idx, "policy idx mismatch at {idx}");
            assert_eq!(rp.is_forbidding, kp.is_forbidding, "is_forbidding mismatch at {idx}");
            assert_eq!(rp.is_conditional, kp.is_conditional, "is_conditional mismatch at {idx}");
        }
    }

    // Action lookup + split_policy_map_for_authorization + entities-of-type,
    // exercised on the real fixtures' action/type.
    let ref_action_policies = reference.get_policies_for_action(fixtures.action).expect("ref action");
    let rkyv_action_policies = SnapshotOps::get_policies_for_action(&rkyv_snap, fixtures.action).expect("rkyv action");
    assert_eq!(ref_action_policies, rkyv_action_policies, "get_policies_for_action mismatch");

    let policy_ids: Vec<u32> = ref_action_policies.iter().collect();
    let ref_split = reference.split_policy_map_for_authorization(&policy_ids);
    let rkyv_split = SnapshotOps::split_policy_map_for_authorization(&rkyv_snap, &policy_ids);
    assert_eq!(ref_split, rkyv_split, "split_policy_map_for_authorization mismatch");

    let ref_targets = reference
        .get_entities_of_type_for_policies(&policy_ids, fixtures.file_type, PolicySide::Resource)
        .expect("ref targets");
    let rkyv_targets = SnapshotOps::get_entities_of_type_for_policies(&rkyv_snap, &policy_ids, fixtures.file_type, PolicySide::Resource)
        .expect("rkyv targets");
    assert_eq!(ref_targets, rkyv_targets, "get_entities_of_type_for_policies mismatch");

    println!("n_entities={n} indices_checked={checked} -- RkyvSnapshot matches Snapshot on every method OK");
}
