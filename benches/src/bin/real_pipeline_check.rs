//! True production-pipeline verification: builds a real `Graph` through the
//! public mutation API, drives it through the *actual* `IndexerService`
//! (rebuild_snapshot -- the real rkyv write path, not a bench script
//! standing in for it) to write a real snapshot file, then loads it through
//! the *actual* `AuthorizerService` (the real rkyv read path) and runs a
//! real check() through the public service API. Confirms the two services
//! are actually compatible now that both go through rkyv, not just that
//! AuthorizerEngine::load_rkyv works in isolation.

use uuid::Uuid;

use arbor_graph_core::graph::Graph;
use arbor_indexer::service::IndexerService;
use arbor_types::{Action, Entity, EntityTypeId, Policy, PolicyTarget, PolicyType};

fn type_id(n: u32) -> EntityTypeId {
    EntityTypeId::new(n)
}

fn main() {
    let mut graph = Graph::new();

    let principal_id = Uuid::new_v4();
    let resource_id = Uuid::new_v4();
    let action_id = Uuid::new_v4();
    let policy_id = Uuid::new_v4();

    graph.upsert_entity(Entity::new(principal_id, "user".into(), type_id(1), vec![])).expect("upsert principal");
    graph.upsert_entity(Entity::new(resource_id, "file".into(), type_id(2), vec![])).expect("upsert resource");
    graph
        .add_action(Action { id: action_id, name: "read".into(), entity_type_id: type_id(99), description: None })
        .expect("add_action");

    graph
        .upsert_policy(Policy::new(
            policy_id,
            "policy".into(),
            None,
            PolicyType::Permit,
            PolicyTarget::Entity(principal_id),
            PolicyTarget::Entity(resource_id),
            vec![action_id],
            vec![],
            None,
        ))
        .expect("upsert_policy");

    let path = std::env::temp_dir().join("real_pipeline_check.rkyv");
    let mut indexer = IndexerService::new(graph, path.clone());
    indexer.rebuild_snapshot().expect("rebuild_snapshot (real indexer write path)");

    let authorizer = arbor_authorizer::service::AuthorizerService::load(&path)
        .expect("AuthorizerService::load (real authorizer read path)");

    // AuthorizerService doesn't expose check() directly (only via gRPC), so
    // reach through its inner engine the same way the gRPC handler does --
    // this is still the real AuthorizerEngine the service actually uses.
    let engine = authorizer.engine();
    let principal_idx = engine.snapshot().uuid_to_index(&principal_id).expect("principal indexed");
    let resource_idx = engine.snapshot().uuid_to_index(&resource_id).expect("resource indexed");
    let action_idx = engine.snapshot().uuid_to_index(&action_id).expect("action indexed");

    let result = engine.check(principal_idx, action_idx, resource_idx).expect("check");

    println!("decision={:?} reason_policy_indices={:?}", result.decision, result.reason_policy_indices);
    assert_eq!(result.decision, arbor_authorizer::engine::Decision::Permit);

    println!("real IndexerService -> AuthorizerService pipeline OK (rkyv end to end)");
}
