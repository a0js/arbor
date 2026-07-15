//! Integration tests for [`SnapshotBuilder`].
//!
//! Each test constructs a [`Graph`] through the public mutation API, builds a
//! [`Snapshot`], and asserts the correctness of the pre-computed indexes.

use arbor_graph_core::graph::Graph;
use arbor_indexer::snapshot_builder::SnapshotBuilder;
use arbor_types::{
    Action, ActionSet, Condition, Entity, EntityTypeId, Operand, Policy, PolicyTarget, PolicyType,
};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Construct a stable `EntityTypeId` from a small integer for test use.
fn type_id(n: u32) -> EntityTypeId {
    EntityTypeId::new(n)
}

/// Create and upsert a plain entity with the given type id and no parents.
fn add_entity(graph: &mut Graph, id: Uuid, type_n: u32) {
    let entity = Entity::new(id, id.to_string(), type_id(type_n), vec![]);
    graph.upsert_entity(entity).expect("upsert_entity failed");
}

/// Create and upsert an entity with one parent UUID.
fn add_entity_with_parent(graph: &mut Graph, id: Uuid, type_n: u32, parent: Uuid) {
    let entity = Entity::new(id, id.to_string(), type_id(type_n), vec![parent]);
    graph.upsert_entity(entity).expect("upsert_entity with parent failed");
}

/// Add a simple permit policy targeting specific entities with a single action.
fn add_permit_policy(
    graph: &mut Graph,
    policy_id: Uuid,
    principal: PolicyTarget,
    resource: PolicyTarget,
    action_id: Uuid,
) {
    let policy = Policy::new(
        policy_id,
        policy_id.to_string(),
        None,
        PolicyType::Permit,
        principal,
        resource,
        vec![action_id],
        vec![],
        None,
    );
    graph.upsert_policy(policy).expect("upsert_policy failed");
}

/// Add a simple action node to the graph and return its graph index.
fn add_action(graph: &mut Graph, action_id: Uuid) -> u32 {
    let action = Action {
        id: action_id,
        name: action_id.to_string(),
        entity_type_id: type_id(99),
        description: None,
    };
    graph.add_action(action).expect("add_action failed");
    *graph.uuid_to_index.get(&action_id).unwrap()
}

// ---------------------------------------------------------------------------
// Test 1: Empty graph
// ---------------------------------------------------------------------------

#[test]
fn test_empty_graph_produces_empty_snapshot() {
    let graph = Graph::new();
    let snapshot = SnapshotBuilder::build(&graph).expect("build should succeed on empty graph");

    assert!(snapshot.uuid_to_index.is_empty(), "uuid_to_index should be empty");
    assert!(snapshot.index_to_uuid.is_empty(), "index_to_uuid should be empty");
    assert!(snapshot.nodes.is_empty(), "nodes should be empty");
    assert!(snapshot.action_to_policies.is_empty(), "action_to_policies should be empty");
    assert!(snapshot.indexed_entity_types.is_empty(), "indexed_entity_types should be empty");
    assert!(snapshot.all_principal_policies.is_empty());
    assert!(snapshot.all_resource_policies.is_empty());
    assert!(snapshot.conditional_policies.is_empty());
    assert!(snapshot.forbidding_policies.is_empty());
    assert!(snapshot.descendant_principal_policies.is_empty());
    assert!(snapshot.descendant_resource_policies.is_empty());
}

// ---------------------------------------------------------------------------
// Test 2: Single entity
// ---------------------------------------------------------------------------

#[test]
fn test_single_entity_indexes() {
    let mut graph = Graph::new();
    let entity_id = Uuid::new_v4();
    add_entity(&mut graph, entity_id, 1);

    let snapshot = SnapshotBuilder::build(&graph).expect("build failed");

    // uuid_to_index and index_to_uuid round-trip
    let idx = *snapshot.uuid_to_index.get(&entity_id).expect("entity uuid missing from index");
    assert_eq!(snapshot.index_to_uuid[idx as usize], Some(entity_id));

    // IndexedEntity present
    let ie = snapshot.get_entity(idx).expect("IndexedEntity missing");
    assert_eq!(ie.entity_type, type_id(1));

    // Self-inclusive: own index must appear in ancestors
    let ancestors = snapshot.ancestors_of(idx).expect("ancestors must resolve");
    assert!(
        ancestors.contains(&idx),
        "entity's own index must be in ancestors bitmap"
    );

    // No parents → ancestors has exactly one entry (self)
    assert_eq!(ancestors.len(), 1);

    // entity type index
    let et = snapshot.indexed_entity_types.get(&type_id(1)).expect("entity type missing");
    assert!(et.nodes_of_type.contains(idx));

    // No policies
    assert!(ie.principal_of_policies.is_none());
    assert!(ie.resource_of_policies.is_none());
}

// ---------------------------------------------------------------------------
// Test 3: Parent-child hierarchy
// ---------------------------------------------------------------------------

#[test]
fn test_parent_child_hierarchy() {
    let mut graph = Graph::new();
    let parent_id = Uuid::new_v4();
    let child_id = Uuid::new_v4();

    add_entity(&mut graph, parent_id, 1);
    add_entity_with_parent(&mut graph, child_id, 1, parent_id);

    let snapshot = SnapshotBuilder::build(&graph).expect("build failed");

    let parent_idx = *snapshot.uuid_to_index.get(&parent_id).unwrap();
    let child_idx = *snapshot.uuid_to_index.get(&child_id).unwrap();

    // Child's ancestors include: self + parent
    let child_ancestors = snapshot.ancestors_of(child_idx).unwrap();
    assert!(child_ancestors.contains(&child_idx), "child must include self");
    assert!(child_ancestors.contains(&parent_idx), "child must include parent");
    assert_eq!(child_ancestors.len(), 2);

    // Parent's ancestors: self only (no grandparents)
    let parent_ancestors = snapshot.ancestors_of(parent_idx).unwrap();
    assert!(parent_ancestors.contains(&parent_idx));
    assert_eq!(parent_ancestors.len(), 1);
}

// ---------------------------------------------------------------------------
// Test 4: Diamond hierarchy  A←B, A←C, B←D, C←D
// ---------------------------------------------------------------------------

#[test]
fn test_diamond_hierarchy() {
    let mut graph = Graph::new();
    let a_id = Uuid::new_v4();
    let b_id = Uuid::new_v4();
    let c_id = Uuid::new_v4();
    let d_id = Uuid::new_v4();

    // Insert in topological order so the graph mutation API is happy
    add_entity(&mut graph, a_id, 1);
    add_entity_with_parent(&mut graph, b_id, 1, a_id);
    add_entity_with_parent(&mut graph, c_id, 1, a_id);

    // D has two parents: B and C
    let entity_d = Entity::new(d_id, d_id.to_string(), type_id(1), vec![b_id, c_id]);
    graph.upsert_entity(entity_d).expect("upsert D failed");

    let snapshot = SnapshotBuilder::build(&graph).expect("build failed");

    let a_idx = *snapshot.uuid_to_index.get(&a_id).unwrap();
    let b_idx = *snapshot.uuid_to_index.get(&b_id).unwrap();
    let c_idx = *snapshot.uuid_to_index.get(&c_id).unwrap();
    let d_idx = *snapshot.uuid_to_index.get(&d_id).unwrap();

    // D's ancestors: D (self), B, C, A
    let d_ancestors = snapshot.ancestors_of(d_idx).unwrap();
    assert!(d_ancestors.contains(&d_idx), "D must contain self");
    assert!(d_ancestors.contains(&b_idx), "D must contain B");
    assert!(d_ancestors.contains(&c_idx), "D must contain C");
    assert!(d_ancestors.contains(&a_idx), "D must contain A");
    assert_eq!(d_ancestors.len(), 4);

    // A has no entity-type policy so effective_principal_policies should be Some(empty).
    let a_ie = snapshot.get_entity(a_idx).unwrap();
    // No policies exist in this graph — effective sets should be Some(empty).
    assert!(a_ie.effective_principal_policies.is_some());
    assert!(a_ie.effective_resource_policies.is_some());
    assert_eq!(snapshot.effective_principal_of(a_idx).len(), 0);
    assert_eq!(snapshot.effective_resource_of(a_idx).len(), 0);
}

// ---------------------------------------------------------------------------
// Test 5: Simple Entity→Entity policy, permit, no condition
// ---------------------------------------------------------------------------

#[test]
fn test_simple_entity_entity_permit_policy() {
    let mut graph = Graph::new();
    let principal_id = Uuid::new_v4();
    let resource_id = Uuid::new_v4();
    let action_id = Uuid::new_v4();
    let policy_id = Uuid::new_v4();

    add_entity(&mut graph, principal_id, 1);
    add_entity(&mut graph, resource_id, 2);
    add_action(&mut graph, action_id);
    add_permit_policy(
        &mut graph,
        policy_id,
        PolicyTarget::Entity(principal_id),
        PolicyTarget::Entity(resource_id),
        action_id,
    );

    let snapshot = SnapshotBuilder::build(&graph).expect("build failed");

    // Policies are NOT in snapshot.uuid_to_index (only entities are).
    // Use graph.uuid_to_index to obtain the graph node index for the policy.
    let policy_idx = *graph.uuid_to_index.get(&policy_id).unwrap();

    let ip = snapshot.get_policy(policy_idx).expect("IndexedPolicy missing");

    assert!(!ip.is_forbidding);
    assert!(!ip.is_conditional);
    assert!(ip.conditions.is_none());

    // Principal entity has this policy in principal_of_policies
    let p_idx = *snapshot.uuid_to_index.get(&principal_id).unwrap();
    assert!(snapshot.principal_of_policies_of(p_idx).contains(&policy_idx));

    // Resource entity has this policy in resource_of_policies
    let r_idx = *snapshot.uuid_to_index.get(&resource_id).unwrap();
    assert!(snapshot.resource_of_policies_of(r_idx).contains(&policy_idx));

    // Not in the "all" bitmaps (those are for PolicyTarget::All only)
    assert!(!snapshot.all_principal_policies.contains(policy_idx));
    assert!(!snapshot.all_resource_policies.contains(policy_idx));

    // Not conditional, not forbidding
    assert!(!snapshot.conditional_policies.contains(policy_idx));
    assert!(!snapshot.forbidding_policies.contains(policy_idx));

    // Not in descendant bitmaps
    assert!(!snapshot.descendant_principal_policies.contains(policy_idx));
    assert!(!snapshot.descendant_resource_policies.contains(policy_idx));
}

// ---------------------------------------------------------------------------
// Test 6: Forbid policy
// ---------------------------------------------------------------------------

#[test]
fn test_forbid_policy_in_forbidding_bitmap() {
    let mut graph = Graph::new();
    let principal_id = Uuid::new_v4();
    let resource_id = Uuid::new_v4();
    let action_id = Uuid::new_v4();
    let policy_id = Uuid::new_v4();

    add_entity(&mut graph, principal_id, 1);
    add_entity(&mut graph, resource_id, 2);
    add_action(&mut graph, action_id);

    let policy = Policy::new(
        policy_id,
        policy_id.to_string(),
        None,
        PolicyType::Forbid,
        PolicyTarget::Entity(principal_id),
        PolicyTarget::Entity(resource_id),
        vec![action_id],
        vec![],
        None,
    );
    graph.upsert_policy(policy).expect("upsert_policy failed");

    let snapshot = SnapshotBuilder::build(&graph).expect("build failed");

    let policy_idx = *graph.uuid_to_index.get(&policy_id).unwrap();
    let ip = snapshot.get_policy(policy_idx).unwrap();

    assert!(ip.is_forbidding);
    assert!(snapshot.forbidding_policies.contains(policy_idx));
    assert!(!snapshot.conditional_policies.contains(policy_idx));
}

// ---------------------------------------------------------------------------
// Test 7: Policy with EntityWithDescendants principal
// ---------------------------------------------------------------------------

#[test]
fn test_entity_with_descendants_principal() {
    let mut graph = Graph::new();
    let principal_id = Uuid::new_v4();
    let resource_id = Uuid::new_v4();
    let action_id = Uuid::new_v4();
    let policy_id = Uuid::new_v4();

    add_entity(&mut graph, principal_id, 1);
    add_entity(&mut graph, resource_id, 2);
    add_action(&mut graph, action_id);

    let policy = Policy::new(
        policy_id,
        policy_id.to_string(),
        None,
        PolicyType::Permit,
        PolicyTarget::EntityWithDescendants(principal_id),
        PolicyTarget::Entity(resource_id),
        vec![action_id],
        vec![],
        None,
    );
    graph.upsert_policy(policy).expect("upsert_policy failed");

    let snapshot = SnapshotBuilder::build(&graph).expect("build failed");

    let policy_idx = *graph.uuid_to_index.get(&policy_id).unwrap();
    let ip = snapshot.get_policy(policy_idx).unwrap();

    assert!(!ip.is_forbidding);
    assert!(!ip.is_conditional);

    // Must appear in descendant_principal_policies
    assert!(snapshot.descendant_principal_policies.contains(policy_idx));
    // Must NOT appear in descendant_resource_policies
    assert!(!snapshot.descendant_resource_policies.contains(policy_idx));

    // Principal entity must still own the policy (root of the subtree)
    let p_idx = *snapshot.uuid_to_index.get(&principal_id).unwrap();
    assert!(snapshot.principal_of_policies_of(p_idx).contains(&policy_idx));

    // descendants_by_target must have an entry for principal_id (EntityWithDescendants
    // principal target) -- principal_id has no children so its bitmap is empty but present.
    assert!(
        snapshot.descendants_by_target.contains_key(&p_idx),
        "descendants_by_target must have an entry for the EntityWithDescendants principal target"
    );
    // resource target is Entity, not EntityWithDescendants → resource_id must NOT be a key.
    let r_idx = *snapshot.uuid_to_index.get(&resource_id).unwrap();
    assert!(
        !snapshot.descendants_by_target.contains_key(&r_idx),
        "descendants_by_target must not have an entry for an Entity (non-descendants) target"
    );

    // The principal entity's effective_principal_policies must contain the policy
    assert!(
        snapshot.effective_principal_of(p_idx).contains(&policy_idx),
        "effective_principal_policies must contain the policy"
    );
}

// ---------------------------------------------------------------------------
// Test 8: Policy with EntityType target
// ---------------------------------------------------------------------------

#[test]
fn test_entity_type_policy_target() {
    let mut graph = Graph::new();
    let resource_id = Uuid::new_v4();
    let action_id = Uuid::new_v4();
    let policy_id = Uuid::new_v4();
    let user_type = type_id(42);

    add_entity(&mut graph, resource_id, 2);
    add_action(&mut graph, action_id);

    let policy = Policy::new(
        policy_id,
        policy_id.to_string(),
        None,
        PolicyType::Permit,
        PolicyTarget::EntityType(user_type),
        PolicyTarget::Entity(resource_id),
        vec![action_id],
        vec![],
        None,
    );
    graph.upsert_policy(policy).expect("upsert_policy failed");

    let snapshot = SnapshotBuilder::build(&graph).expect("build failed");

    let policy_idx = *graph.uuid_to_index.get(&policy_id).unwrap();

    // Entity type index must reflect the policy
    let et = snapshot
        .indexed_entity_types
        .get(&user_type)
        .expect("entity type missing from indexed_entity_types");
    assert!(
        et.policies_targeting_principals_of_type.contains(policy_idx),
        "policy must appear in policies_targeting_principals_of_type"
    );
    // Resource side is EntityTarget::Entity, not EntityType → must NOT appear in resource targeting
    assert!(!et.policies_targeting_resources_of_type.contains(policy_idx));

    // Not in all_principal_policies (that's for PolicyTarget::All)
    assert!(!snapshot.all_principal_policies.contains(policy_idx));
}

// ---------------------------------------------------------------------------
// Test 9: Policy with All target
// ---------------------------------------------------------------------------

#[test]
fn test_all_target_policies() {
    let mut graph = Graph::new();
    let resource_id = Uuid::new_v4();
    let action_id = Uuid::new_v4();
    let policy_id = Uuid::new_v4();

    add_entity(&mut graph, resource_id, 2);
    add_action(&mut graph, action_id);

    let policy = Policy::new(
        policy_id,
        policy_id.to_string(),
        None,
        PolicyType::Permit,
        PolicyTarget::All,
        PolicyTarget::All,
        vec![action_id],
        vec![],
        None,
    );
    graph.upsert_policy(policy).expect("upsert_policy failed");

    let snapshot = SnapshotBuilder::build(&graph).expect("build failed");

    let policy_idx = *graph.uuid_to_index.get(&policy_id).unwrap();

    assert!(
        snapshot.all_principal_policies.contains(policy_idx),
        "all_principal_policies must contain policy"
    );
    assert!(
        snapshot.all_resource_policies.contains(policy_idx),
        "all_resource_policies must contain policy"
    );
    // Not in descendant or entity-type bitmaps
    assert!(!snapshot.descendant_principal_policies.contains(policy_idx));
    assert!(!snapshot.descendant_resource_policies.contains(policy_idx));
}

// ---------------------------------------------------------------------------
// Test 10: Action expansion and action set expansion
// ---------------------------------------------------------------------------

#[test]
fn test_action_and_action_set_expansion() {
    let mut graph = Graph::new();
    let principal_id = Uuid::new_v4();
    let resource_id = Uuid::new_v4();
    let action1_id = Uuid::new_v4();
    let action2_id = Uuid::new_v4();
    let action3_id = Uuid::new_v4();
    let action_set_id = Uuid::new_v4();
    let policy_id = Uuid::new_v4();

    add_entity(&mut graph, principal_id, 1);
    add_entity(&mut graph, resource_id, 2);

    // Three independent actions
    let act1_idx = add_action(&mut graph, action1_id);
    let _act2_idx = add_action(&mut graph, action2_id);
    let act3_idx = add_action(&mut graph, action3_id);

    // Action set contains action2 and action3
    let action_set = ActionSet {
        id: action_set_id,
        name: action_set_id.to_string(),
        description: None,
        actions: vec![action2_id, action3_id],
    };
    graph.upsert_action_set(action_set).expect("upsert_action_set failed");
    let act2_idx = *graph.uuid_to_index.get(&action2_id).unwrap();
    let as_idx = *graph.uuid_to_index.get(&action_set_id).unwrap();

    // Policy references action1 directly and the action set
    let policy = Policy::new(
        policy_id,
        policy_id.to_string(),
        None,
        PolicyType::Permit,
        PolicyTarget::Entity(principal_id),
        PolicyTarget::Entity(resource_id),
        vec![action1_id],        // direct action
        vec![action_set_id],     // action set
        None,
    );
    graph.upsert_policy(policy).expect("upsert_policy failed");

    let snapshot = SnapshotBuilder::build(&graph).expect("build failed");
    let policy_idx = *graph.uuid_to_index.get(&policy_id).unwrap();

    // action_to_policies reverse index
    let a1_policies = snapshot.action_to_policies.get(&act1_idx).unwrap();
    assert!(a1_policies.contains(policy_idx));

    let a2_policies = snapshot.action_to_policies.get(&act2_idx).unwrap();
    assert!(a2_policies.contains(policy_idx));

    let a3_policies = snapshot.action_to_policies.get(&act3_idx).unwrap();
    assert!(a3_policies.contains(policy_idx));

    // The action set node index should NOT be in action_to_policies
    assert!(
        !snapshot.action_to_policies.contains_key(&as_idx),
        "action set index must not appear in action_to_policies"
    );
}

// ---------------------------------------------------------------------------
// Test 11: Policy with a condition
// ---------------------------------------------------------------------------

#[test]
fn test_policy_with_condition() {
    let mut graph = Graph::new();
    let principal_id = Uuid::new_v4();
    let resource_id = Uuid::new_v4();
    let action_id = Uuid::new_v4();
    let policy_id = Uuid::new_v4();

    add_entity(&mut graph, principal_id, 1);
    add_entity(&mut graph, resource_id, 2);
    add_action(&mut graph, action_id);

    // A simple, always-compilable condition: 1 == 1
    let condition = Condition::Eq(Operand::Integer(1), Operand::Integer(1));

    let policy = Policy::new(
        policy_id,
        policy_id.to_string(),
        None,
        PolicyType::Permit,
        PolicyTarget::Entity(principal_id),
        PolicyTarget::Entity(resource_id),
        vec![action_id],
        vec![],
        Some(condition),
    );
    graph.upsert_policy(policy).expect("upsert_policy failed");

    let snapshot = SnapshotBuilder::build(&graph).expect("build failed");
    let policy_idx = *graph.uuid_to_index.get(&policy_id).unwrap();

    let ip = snapshot.get_policy(policy_idx).unwrap();

    assert!(ip.is_conditional, "policy must be flagged as conditional");
    assert!(ip.conditions.is_some(), "compiled condition must be Some");
    assert!(
        snapshot.conditional_policies.contains(policy_idx),
        "conditional_policies bitmap must contain this policy"
    );
    // Not forbidding
    assert!(!snapshot.forbidding_policies.contains(policy_idx));
}

// ---------------------------------------------------------------------------
// Test 12: Multiple policies — correct isolation
// ---------------------------------------------------------------------------

#[test]
fn test_multiple_policies_isolated() {
    let mut graph = Graph::new();
    let alice_id = Uuid::new_v4();
    let bob_id = Uuid::new_v4();
    let doc_id = Uuid::new_v4();
    let action_id = Uuid::new_v4();
    let permit_id = Uuid::new_v4();
    let forbid_id = Uuid::new_v4();

    add_entity(&mut graph, alice_id, 1);
    add_entity(&mut graph, bob_id, 1);
    add_entity(&mut graph, doc_id, 2);
    add_action(&mut graph, action_id);

    // Alice gets a permit
    add_permit_policy(
        &mut graph,
        permit_id,
        PolicyTarget::Entity(alice_id),
        PolicyTarget::Entity(doc_id),
        action_id,
    );

    // Bob gets a forbid
    let forbid = Policy::new(
        forbid_id,
        forbid_id.to_string(),
        None,
        PolicyType::Forbid,
        PolicyTarget::Entity(bob_id),
        PolicyTarget::Entity(doc_id),
        vec![action_id],
        vec![],
        None,
    );
    graph.upsert_policy(forbid).expect("upsert_policy failed");

    let snapshot = SnapshotBuilder::build(&graph).expect("build failed");

    let permit_idx = *graph.uuid_to_index.get(&permit_id).unwrap();
    let forbid_idx = *graph.uuid_to_index.get(&forbid_id).unwrap();

    // Forbid policy in forbidding_policies; permit is not
    assert!(snapshot.forbidding_policies.contains(forbid_idx));
    assert!(!snapshot.forbidding_policies.contains(permit_idx));

    // Alice's principal_of_policies has permit but not forbid
    let alice_idx = *snapshot.uuid_to_index.get(&alice_id).unwrap();
    let alice_pols = snapshot.principal_of_policies_of(alice_idx);
    assert!(alice_pols.contains(&permit_idx));
    assert!(!alice_pols.contains(&forbid_idx));

    // Bob's principal_of_policies has forbid but not permit
    let bob_idx = *snapshot.uuid_to_index.get(&bob_id).unwrap();
    let bob_pols = snapshot.principal_of_policies_of(bob_idx);
    assert!(bob_pols.contains(&forbid_idx));
    assert!(!bob_pols.contains(&permit_idx));
}

// ---------------------------------------------------------------------------
// Test 13: index_to_uuid length and coverage
// ---------------------------------------------------------------------------

#[test]
fn test_index_to_uuid_coverage() {
    let mut graph = Graph::new();
    let e1 = Uuid::new_v4();
    let e2 = Uuid::new_v4();

    add_entity(&mut graph, e1, 1);
    add_entity(&mut graph, e2, 1);

    let snapshot = SnapshotBuilder::build(&graph).expect("build failed");

    // index_to_uuid length must equal graph.next_index
    assert_eq!(snapshot.index_to_uuid.len(), graph.next_index as usize);

    let idx1 = *snapshot.uuid_to_index.get(&e1).unwrap();
    let idx2 = *snapshot.uuid_to_index.get(&e2).unwrap();

    assert_eq!(snapshot.index_to_uuid[idx1 as usize], Some(e1));
    assert_eq!(snapshot.index_to_uuid[idx2 as usize], Some(e2));
}

// ---------------------------------------------------------------------------
// Test 14: EntityWithDescendants resource target
// ---------------------------------------------------------------------------

#[test]
fn test_entity_with_descendants_resource() {
    let mut graph = Graph::new();
    let principal_id = Uuid::new_v4();
    let root_resource_id = Uuid::new_v4();
    let action_id = Uuid::new_v4();
    let policy_id = Uuid::new_v4();

    add_entity(&mut graph, principal_id, 1);
    add_entity(&mut graph, root_resource_id, 2);
    add_action(&mut graph, action_id);

    let policy = Policy::new(
        policy_id,
        policy_id.to_string(),
        None,
        PolicyType::Permit,
        PolicyTarget::Entity(principal_id),
        PolicyTarget::EntityWithDescendants(root_resource_id),
        vec![action_id],
        vec![],
        None,
    );
    graph.upsert_policy(policy).expect("upsert_policy failed");

    let snapshot = SnapshotBuilder::build(&graph).expect("build failed");
    let policy_idx = *graph.uuid_to_index.get(&policy_id).unwrap();

    // Must appear in descendant_resource_policies
    assert!(snapshot.descendant_resource_policies.contains(policy_idx));
    // Not descendant_principal_policies
    assert!(!snapshot.descendant_principal_policies.contains(policy_idx));

    // Root resource entity must own the policy in resource_of_policies
    let r_idx = *snapshot.uuid_to_index.get(&root_resource_id).unwrap();
    assert!(snapshot.resource_of_policies_of(r_idx).contains(&policy_idx));

    // descendants_by_target must have an entry for root_resource_id (EntityWithDescendants
    // resource target).
    assert!(
        snapshot.descendants_by_target.contains_key(&r_idx),
        "descendants_by_target must have an entry for the EntityWithDescendants resource target"
    );
    // principal target is Entity, not EntityWithDescendants → principal_id must NOT be a key.
    let p_idx = *snapshot.uuid_to_index.get(&principal_id).unwrap();
    assert!(
        !snapshot.descendants_by_target.contains_key(&p_idx),
        "descendants_by_target must not have an entry for an Entity (non-descendants) target"
    );

    // The resource entity's effective_resource_policies must contain the policy
    assert!(
        snapshot.effective_resource_of(r_idx).contains(&policy_idx),
        "effective_resource_policies must contain the policy"
    );
}

// ---------------------------------------------------------------------------
// Test 15: Entity type nodes_of_type covers all entities of a type
// ---------------------------------------------------------------------------

#[test]
fn test_entity_type_nodes_of_type() {
    let mut graph = Graph::new();
    let user_type = type_id(5);
    let doc_type = type_id(6);

    let u1 = Uuid::new_v4();
    let u2 = Uuid::new_v4();
    let d1 = Uuid::new_v4();

    let u1_entity = Entity::new(u1, u1.to_string(), user_type, vec![]);
    let u2_entity = Entity::new(u2, u2.to_string(), user_type, vec![]);
    let d1_entity = Entity::new(d1, d1.to_string(), doc_type, vec![]);

    graph.upsert_entity(u1_entity).unwrap();
    graph.upsert_entity(u2_entity).unwrap();
    graph.upsert_entity(d1_entity).unwrap();

    let snapshot = SnapshotBuilder::build(&graph).expect("build failed");

    let u1_idx = *snapshot.uuid_to_index.get(&u1).unwrap();
    let u2_idx = *snapshot.uuid_to_index.get(&u2).unwrap();
    let d1_idx = *snapshot.uuid_to_index.get(&d1).unwrap();

    let user_et = snapshot.indexed_entity_types.get(&user_type).unwrap();
    assert!(user_et.nodes_of_type.contains(u1_idx));
    assert!(user_et.nodes_of_type.contains(u2_idx));
    assert!(!user_et.nodes_of_type.contains(d1_idx));
    assert_eq!(user_et.nodes_of_type.len(), 2);

    let doc_et = snapshot.indexed_entity_types.get(&doc_type).unwrap();
    assert!(doc_et.nodes_of_type.contains(d1_idx));
    assert_eq!(doc_et.nodes_of_type.len(), 1);
}

// ---------------------------------------------------------------------------
// Test 16: effective_resource_policies smoke test
// ---------------------------------------------------------------------------

#[test]
fn test_effective_resource_policies() {
    let mut graph = Graph::new();
    let principal_id = Uuid::new_v4();
    let resource_id = Uuid::new_v4();
    let action_id = Uuid::new_v4();
    let policy_id = Uuid::new_v4();

    add_entity(&mut graph, principal_id, 1);
    add_entity(&mut graph, resource_id, 2);
    add_action(&mut graph, action_id);
    add_permit_policy(
        &mut graph,
        policy_id,
        PolicyTarget::Entity(principal_id),
        PolicyTarget::Entity(resource_id),
        action_id,
    );

    let snapshot = SnapshotBuilder::build(&graph).expect("build failed");

    let r_idx = *snapshot.uuid_to_index.get(&resource_id).unwrap();
    let policy_idx = *graph.uuid_to_index.get(&policy_id).unwrap();

    let policies = snapshot.effective_resource_of(r_idx);
    assert!(policies.contains(&policy_idx), "policy must appear in resource's effective policy set");
}

// ---------------------------------------------------------------------------
// Test 17: Condition compile failure → is_conditional=true, conditions=None
//
// We force a compile error by supplying a `String` literal as the left-hand
// operand of `InNetwork` — the compiler requires that side to be an IpAddr or
// Variable and returns `CompileError::InvalidOperand` for anything else.
// ---------------------------------------------------------------------------

#[test]
fn test_compile_failure_is_safe() {
    use ipnet::IpNet;

    let mut graph = Graph::new();
    let principal_id = Uuid::new_v4();
    let resource_id = Uuid::new_v4();
    let action_id = Uuid::new_v4();
    let policy_id = Uuid::new_v4();

    add_entity(&mut graph, principal_id, 1);
    add_entity(&mut graph, resource_id, 2);
    add_action(&mut graph, action_id);

    // `InNetwork` with a String literal on the left fails with InvalidOperand.
    let bad_network: IpNet = "10.0.0.0/8".parse().unwrap();
    let condition = Condition::InNetwork(
        Operand::String("not-an-ip".to_string()),
        Operand::IpNetwork(bad_network),
    );

    let policy = Policy::new(
        policy_id,
        policy_id.to_string(),
        None,
        PolicyType::Permit,
        PolicyTarget::Entity(principal_id),
        PolicyTarget::Entity(resource_id),
        vec![action_id],
        vec![],
        Some(condition),
    );
    graph.upsert_policy(policy).expect("upsert_policy failed");

    let snapshot = SnapshotBuilder::build(&graph).expect("build must succeed despite compile error");
    let policy_idx = *graph.uuid_to_index.get(&policy_id).unwrap();

    let ip = snapshot.get_policy(policy_idx).unwrap();

    // Even though compilation failed the policy must still be flagged conditional
    assert!(ip.is_conditional, "is_conditional must be true even when compile fails");
    assert!(ip.conditions.is_none(), "conditions must be None on compile failure");
    assert!(
        snapshot.conditional_policies.contains(policy_idx),
        "conditional_policies must include this policy"
    );
}
