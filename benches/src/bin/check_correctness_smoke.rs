//! Direct correctness check on `AuthorizerEngine::check()`'s actual Decision
//! output after rewriting `split_policy_map_for_authorization` to classify
//! via array iteration instead of RoaringBitmap AND/SUB -- confirms the
//! forbidding/permitting, conditional/unconditional bucket classification
//! wasn't silently swapped or inverted, not just that it compiles and times
//! reasonably.
//!
//! Verified against the pre-refactor code in an isolated worktree: `Permit`
//! for `denied_principal` on `read` is pre-existing behavior (it matches the
//! global `EntityType(User) -> EntityType(File), read` permit regardless of
//! hierarchy) -- not a regression. `delete_action` exercises a genuine
//! forbid-with-no-overriding-permit case instead.
//!
//! Usage: check_correctness_smoke <n_entities>

use std::env;

use arbor_authorizer::engine::{AuthorizerEngine, Decision};
use arbor_bench::build_scenario;

fn main() {
    let n: usize = env::args()
        .nth(1)
        .expect("usage: check_correctness_smoke <n_entities>")
        .parse()
        .expect("n_entities must be a positive integer");

    let (snapshot, fixtures) = build_scenario(n);
    let engine = AuthorizerEngine::from_snapshot(snapshot);

    let permitted_read = engine
        .check(fixtures.permitted_principal, fixtures.action, fixtures.resource)
        .expect("check failed for permitted_principal/read");
    // denied_principal still matches the global type-wide read permit --
    // see BenchFixtures::denied_principal doc comment.
    let denied_read = engine
        .check(fixtures.denied_principal, fixtures.action, fixtures.resource)
        .expect("check failed for denied_principal/read");
    // Genuine forbid case: policy 4 forbids delete for EntityType(User) ->
    // EntityType(File), and nothing overrides it for a plain user.
    let denied_delete = engine
        .check(fixtures.denied_principal, fixtures.delete_action, fixtures.resource)
        .expect("check failed for denied_principal/delete");

    println!("permitted_principal/read -> {:?} (reasons: {:?})", permitted_read.decision, permitted_read.reason_policy_indices);
    println!("denied_principal/read    -> {:?} (reasons: {:?})", denied_read.decision, denied_read.reason_policy_indices);
    println!("denied_principal/delete  -> {:?} (reasons: {:?})", denied_delete.decision, denied_delete.reason_policy_indices);

    for &idx in &denied_delete.reason_policy_indices {
        let p = engine.snapshot().get_policy(idx).expect("policy must exist");
        println!(
            "  policy[{idx}]: is_forbidding={} is_conditional={} principal_target={:?} resource_target={:?}",
            p.is_forbidding, p.is_conditional, p.principal_target, p.resource_target
        );
    }

    assert_eq!(permitted_read.decision, Decision::Permit, "permitted_principal/read must be Permit");
    assert!(!permitted_read.reason_policy_indices.is_empty(), "Permit must cite a reason policy");

    assert_eq!(denied_read.decision, Decision::Permit, "denied_principal/read matches the global type-wide grant");

    assert_eq!(denied_delete.decision, Decision::Deny, "denied_principal/delete must be Deny (forbid, no override)");
    assert!(
        denied_delete.reason_policy_indices.iter().any(|&idx| engine.snapshot().get_policy(idx).unwrap().is_forbidding),
        "Deny reason must cite the forbidding policy"
    );

    println!("OK: all three decisions correct");
}
