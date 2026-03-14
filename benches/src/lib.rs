//! Benchmark data generator for the Arbor authorization system.
//!
//! [`build_scenario`] constructs a deterministic graph at three scales
//! (100k / 1M / 2M entities) and returns the compiled [`Snapshot`] together
//! with a [`BenchFixtures`] struct that identifies the interesting indices
//! used by the benchmark cases.

use arbor_graph_core::graph::Graph;
use arbor_index_snapshot::Snapshot;
use arbor_indexer::snapshot_builder::SnapshotBuilder;
use arbor_types::{
    Action, AttributeNameId, AttributeValue, Condition, Entity, EntityTypeId, Operand, Policy,
    PolicyTarget, PolicyType, VariableRef, VariableScope,
};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Fixed UUID namespaces — all derived deterministically from a single root.
// ---------------------------------------------------------------------------

const NS_ROOT: Uuid = Uuid::from_u128(0x6ba7_b814_9dad_11d1_80b4_00c0_4fd4_30c8);

fn entity_uuid(label: &str, i: usize) -> Uuid {
    let key = format!("{label}-{i}");
    Uuid::new_v5(&NS_ROOT, key.as_bytes())
}

fn policy_uuid(label: &str, i: usize) -> Uuid {
    let key = format!("policy-{label}-{i}");
    Uuid::new_v5(&NS_ROOT, key.as_bytes())
}

fn action_uuid(name: &str) -> Uuid {
    Uuid::new_v5(&NS_ROOT, name.as_bytes())
}

// ---------------------------------------------------------------------------
// BenchFixtures
// ---------------------------------------------------------------------------

/// Snapshot indices used by the authorization benchmarks.
pub struct BenchFixtures {
    /// A user that has a matching permit policy — `check()` should return Permit.
    pub permitted_principal: u32,
    /// A user with no matching policy — `check()` should return Deny.
    pub denied_principal: u32,
    /// A deeply-nested file node that exercises the ancestor walk.
    pub resource: u32,
    /// Index of the "read" action.
    pub action: u32,
    /// `EntityTypeId` for the "file" type, used in `list_entities`.
    pub file_type: EntityTypeId,
}

// ---------------------------------------------------------------------------
// build_scenario
// ---------------------------------------------------------------------------

/// Build a deterministic authorization scenario at `n_entities` scale.
///
/// # Entity ratios
///
/// | Kind    | Fraction |
/// |---------|----------|
/// | Users   | ~50 %    |
/// | Groups  | ~5 %     |
/// | Files   | ~40 %    |
/// | Folders | ~5 %     |
///
/// # Policy set (fixed, independent of scale)
///
/// 1. **Permit** — `EntityWithDescendants(top-org)` → `EntityWithDescendants(root-folder)` — `read`
/// 2. **Permit** — `EntityType(User)` → `EntityType(File)` — `read`
/// 3. **Permit** — `Entity(specific-user)` → `Entity(specific-file)` — `read`
/// 4. **Forbid** — `EntityType(User)` → `EntityType(File)` — `delete`
///
/// # Returned fixtures
///
/// * `permitted_principal` — the specific user named in policy 3.
/// * `denied_principal`    — a user outside the org hierarchy (no policy match).
/// * `resource`            — the deepest file in the folder tree.
/// * `action`              — the "read" action index.
/// * `file_type`           — the `EntityTypeId` for files.
pub fn build_scenario(n_entities: usize) -> (Snapshot, BenchFixtures) {
    let n_users = (n_entities * 50) / 100;
    let n_groups = (n_entities * 5) / 100;
    let n_files = (n_entities * 40) / 100;
    let n_folders = n_entities - n_users - n_groups - n_files;

    let mut graph = Graph::new();

    // Register entity types up front so IDs are stable.
    let user_type = graph.get_or_create_entity_type_id("User");
    let group_type = graph.get_or_create_entity_type_id("Group");
    let file_type = graph.get_or_create_entity_type_id("File");
    let folder_type = graph.get_or_create_entity_type_id("Folder");

    // Attribute name IDs — stable u32 constants, no registry needed.
    let attr_department  = AttributeNameId::new(1); // e.g. "dept-0", "dept-1", ...
    let attr_clearance   = AttributeNameId::new(2); // 0..4
    let attr_sensitivity = AttributeNameId::new(3); // 0..4 (on files)

    // ------------------------------------------------------------------
    // Actions
    // ------------------------------------------------------------------
    let read_id = action_uuid("read");
    let delete_id = action_uuid("delete");

    graph.add_action(Action { id: read_id, name: "read".into(), entity_type_id: file_type, description: None })
        .expect("add read action");
    graph.add_action(Action { id: delete_id, name: "delete".into(), entity_type_id: file_type, description: None })
        .expect("add delete action");

    // ------------------------------------------------------------------
    // Principal hierarchy: org → depts → teams → users (4 levels)
    //
    //   1 org
    //   ~4 depts (children of org)
    //   ~floor(n_groups/4) teams per dept
    //   remaining users spread across teams
    // ------------------------------------------------------------------

    let org_id = entity_uuid("org", 0);
    graph.upsert_entity(Entity::new(org_id, "org-0".into(), group_type, vec![]))
        .expect("upsert org");

    let n_depts = 4usize.min(n_groups);
    let n_teams = n_groups.saturating_sub(n_depts);

    let mut dept_ids = Vec::with_capacity(n_depts);
    for i in 0..n_depts {
        let id = entity_uuid("dept", i);
        graph.upsert_entity(Entity::new(id, format!("dept-{i}"), group_type, vec![org_id]))
            .expect("upsert dept");
        dept_ids.push(id);
    }

    let mut team_ids = Vec::with_capacity(n_teams);
    for i in 0..n_teams {
        let parent_dept = dept_ids[i % n_depts];
        let id = entity_uuid("team", i);
        graph.upsert_entity(Entity::new(id, format!("team-{i}"), group_type, vec![parent_dept]))
            .expect("upsert team");
        team_ids.push(id);
    }

    // Specific (permitted) user lives in team-0 (or dept-0 if no teams).
    let permitted_user_id = entity_uuid("user", 0);
    let permitted_user_parent = if !team_ids.is_empty() {
        team_ids[0]
    } else if !dept_ids.is_empty() {
        dept_ids[0]
    } else {
        org_id
    };
    let mut permitted_user = Entity::new(permitted_user_id, "user-0".into(), user_type, vec![permitted_user_parent]);
    permitted_user.add_attribute(attr_department, AttributeValue::String("dept-0".into()));
    permitted_user.add_attribute(attr_clearance, AttributeValue::Integer(3));
    graph.upsert_entity(permitted_user).expect("upsert permitted user");

    // Rest of users distributed across teams (or dept fallback), with attributes.
    for i in 1..n_users {
        let parent = if !team_ids.is_empty() {
            team_ids[i % team_ids.len()]
        } else if !dept_ids.is_empty() {
            dept_ids[i % dept_ids.len()]
        } else {
            org_id
        };
        let id = entity_uuid("user", i);
        let mut user = Entity::new(id, format!("user-{i}"), user_type, vec![parent]);
        user.add_attribute(attr_department, AttributeValue::String(format!("dept-{}", i % n_depts.max(1))));
        user.add_attribute(attr_clearance, AttributeValue::Integer((i % 5) as i64));
        graph.upsert_entity(user).expect("upsert user");
    }

    // Denied user — sits outside the org (no parent), clearance 0.
    let denied_user_id = entity_uuid("denied-user", 0);
    let mut denied_user = Entity::new(denied_user_id, "denied-user-0".into(), user_type, vec![]);
    denied_user.add_attribute(attr_department, AttributeValue::String("external".into()));
    denied_user.add_attribute(attr_clearance, AttributeValue::Integer(0));
    graph.upsert_entity(denied_user).expect("upsert denied user");

    // ------------------------------------------------------------------
    // Resource hierarchy: recursive folder tree, files as leaves.
    //
    // Branching factor 2-3 capped at n_folders budget.
    // We build the tree level by level with branching factor 2.
    // ------------------------------------------------------------------

    let root_folder_id = entity_uuid("folder", 0);
    graph.upsert_entity(Entity::new(root_folder_id, "folder-0".into(), folder_type, vec![]))
        .expect("upsert root folder");

    let mut folder_ids: Vec<Uuid> = vec![root_folder_id];
    let mut folder_cursor = 1usize;  // next folder index

    // Level-order construction: expand each folder with up to 2 children
    // until we hit the folder budget.
    let mut expand_queue: Vec<Uuid> = vec![root_folder_id];
    let mut eq_head = 0usize;
    while folder_cursor < n_folders {
        if eq_head >= expand_queue.len() {
            // The tree filled up; restart from root for remaining folders.
            eq_head = 0;
        }
        let parent_folder = expand_queue[eq_head];
        eq_head += 1;

        for _child in 0..2 {
            if folder_cursor >= n_folders {
                break;
            }
            let id = entity_uuid("folder", folder_cursor);
            graph.upsert_entity(Entity::new(
                id,
                format!("folder-{folder_cursor}"),
                folder_type,
                vec![parent_folder],
            )).expect("upsert folder");
            expand_queue.push(id);
            folder_ids.push(id);
            folder_cursor += 1;
        }
    }

    // Distribute files across leaf folders (last third of the folder list).
    let leaf_start = folder_ids.len().saturating_sub(folder_ids.len() / 3 + 1);
    let leaf_count = folder_ids.len() - leaf_start;

    // Specific (deep) file: last file lives under the last folder in expand_queue
    // (deepest reachable via the tree expansion).
    let deepest_folder_id = *expand_queue.last().unwrap_or(&root_folder_id);
    let specific_file_id = entity_uuid("file", 0);
    let mut specific_file = Entity::new(specific_file_id, "file-0".into(), file_type, vec![deepest_folder_id]);
    specific_file.add_attribute(attr_sensitivity, AttributeValue::Integer(3));
    graph.upsert_entity(specific_file).expect("upsert specific file");

    for i in 1..n_files {
        let parent_folder = if leaf_count > 0 {
            folder_ids[leaf_start + (i % leaf_count)]
        } else {
            root_folder_id
        };
        let id = entity_uuid("file", i);
        let mut file = Entity::new(id, format!("file-{i}"), file_type, vec![parent_folder]);
        file.add_attribute(attr_sensitivity, AttributeValue::Integer((i % 5) as i64));
        graph.upsert_entity(file).expect("upsert file");
    }

    // ------------------------------------------------------------------
    // Policies
    //
    // Scaled to ~1 policy per 75 entities to reflect realistic deployments.
    //
    // Distribution:
    //   Base (4)        — type-level and org-level grants/forbids
    //   Team grants     — 40% of budget: EntityWithDescendants(team) → EntityWithDescendants(folder), read
    //   Folder grants   — 30% of budget: EntityWithDescendants(team) → Entity(folder), read
    //   User overrides  — 20% of budget: Entity(user) → Entity(file), read
    //   Forbids         — 10% of budget: Entity(user) → Entity(file), delete
    // ------------------------------------------------------------------

    // Policy 1: permit EntityWithDescendants(org) → EntityWithDescendants(root-folder), read.
    graph.upsert_policy(Policy::new(
        policy_uuid("org-root-read", 0),
        "org-root-read".into(),
        None,
        PolicyType::Permit,
        PolicyTarget::EntityWithDescendants(org_id),
        PolicyTarget::EntityWithDescendants(root_folder_id),
        vec![read_id],
        vec![],
        None,
    )).expect("upsert policy 1");

    // Policy 2: permit EntityType(User) → EntityType(File), read.
    graph.upsert_policy(Policy::new(
        policy_uuid("user-file-read", 0),
        "user-file-read".into(),
        None,
        PolicyType::Permit,
        PolicyTarget::EntityType(user_type),
        PolicyTarget::EntityType(file_type),
        vec![read_id],
        vec![],
        None,
    )).expect("upsert policy 2");

    // Policy 3: permit Entity(permitted-user) → Entity(specific-file), read.
    graph.upsert_policy(Policy::new(
        policy_uuid("specific-user-file", 0),
        "specific-user-file".into(),
        None,
        PolicyType::Permit,
        PolicyTarget::Entity(permitted_user_id),
        PolicyTarget::Entity(specific_file_id),
        vec![read_id],
        vec![],
        None,
    )).expect("upsert policy 3");

    // Policy 4: forbid EntityType(User) → EntityType(File), delete.
    graph.upsert_policy(Policy::new(
        policy_uuid("user-file-delete-forbid", 0),
        "user-file-delete-forbid".into(),
        None,
        PolicyType::Forbid,
        PolicyTarget::EntityType(user_type),
        PolicyTarget::EntityType(file_type),
        vec![delete_id],
        vec![],
        None,
    )).expect("upsert policy 4");

    // Scaled policies — target ~1 policy per 75 entities beyond the 4 base ones.
    let policy_budget = (n_entities / 75).saturating_sub(4);
    let n_team_grants   = (policy_budget * 40) / 100;
    let n_folder_grants = (policy_budget * 30) / 100;
    let n_user_overrides = (policy_budget * 20) / 100;
    let n_forbids       = policy_budget - n_team_grants - n_folder_grants - n_user_overrides;

    // Team grants: EntityWithDescendants(team) → EntityWithDescendants(folder), read.
    // Each team gets access to a rotating slice of the folder tree.
    if !team_ids.is_empty() {
        for i in 0..n_team_grants {
            let team = team_ids[i % team_ids.len()];
            let folder = folder_ids[i % folder_ids.len()];
            graph.upsert_policy(Policy::new(
                policy_uuid("team-folder-read", i),
                format!("team-folder-read-{i}"),
                None,
                PolicyType::Permit,
                PolicyTarget::EntityWithDescendants(team),
                PolicyTarget::EntityWithDescendants(folder),
                vec![read_id],
                vec![],
                None,
            )).expect("upsert team grant");
        }
    }

    // Folder grants: EntityWithDescendants(team) → Entity(folder), read.
    // Narrower than team grants — access to a specific folder only, no descendants.
    if !team_ids.is_empty() {
        for i in 0..n_folder_grants {
            let team = team_ids[(i + 1) % team_ids.len()];
            let folder = folder_ids[(i * 3 + 1) % folder_ids.len()];
            graph.upsert_policy(Policy::new(
                policy_uuid("team-specific-folder", i),
                format!("team-specific-folder-{i}"),
                None,
                PolicyType::Permit,
                PolicyTarget::EntityWithDescendants(team),
                PolicyTarget::Entity(folder),
                vec![read_id],
                vec![],
                None,
            )).expect("upsert folder grant");
        }
    }

    // User overrides: Entity(user) → Entity(file), read.
    // Individual users with explicit access to specific files.
    for i in 0..n_user_overrides {
        let user = entity_uuid("user", (i + 1) % n_users.max(1));
        let file = entity_uuid("file", (i * 7 + 3) % n_files.max(1));
        graph.upsert_policy(Policy::new(
            policy_uuid("user-file-override", i),
            format!("user-file-override-{i}"),
            None,
            PolicyType::Permit,
            PolicyTarget::Entity(user),
            PolicyTarget::Entity(file),
            vec![read_id],
            vec![],
            None,
        )).expect("upsert user override");
    }

    // Forbids: Entity(user) → Entity(file), delete.
    // Explicit deny for specific user-file pairs.
    for i in 0..n_forbids {
        let user = entity_uuid("user", (i * 3 + 2) % n_users.max(1));
        let file = entity_uuid("file", (i * 5 + 1) % n_files.max(1));
        graph.upsert_policy(Policy::new(
            policy_uuid("user-file-forbid", i),
            format!("user-file-forbid-{i}"),
            None,
            PolicyType::Forbid,
            PolicyTarget::Entity(user),
            PolicyTarget::Entity(file),
            vec![delete_id],
            vec![],
            None,
        )).expect("upsert forbid");
    }

    // ------------------------------------------------------------------
    // Conditional policies — exercise the bytecode VM at eval time.
    //
    // These sit on top of the scaled set and cover three realistic patterns:
    //
    //   C1. Permit all users WHERE principal.clearance >= resource.sensitivity
    //       — attribute comparison across both sides; most realistic ABAC pattern.
    //
    //   C2. Permit EntityType(User) → EntityType(File) read
    //       WHERE principal.department == "dept-0"
    //       — single-side string equality; faster path, but still hits the VM.
    //
    //   C3. Forbid EntityType(User) → EntityType(File) read
    //       WHERE resource.sensitivity == 4
    //       — conditional forbid; ensures the VM is exercised on the deny path too.
    // ------------------------------------------------------------------

    // C1: clearance >= sensitivity
    graph.upsert_policy(Policy::new(
        policy_uuid("conditional-clearance", 0),
        "conditional-clearance".into(),
        None,
        PolicyType::Permit,
        PolicyTarget::EntityType(user_type),
        PolicyTarget::EntityType(file_type),
        vec![read_id],
        vec![],
        Some(Condition::Gte(
            Operand::Variable(VariableRef { scope: VariableScope::Principal, path: vec![attr_clearance] }),
            Operand::Variable(VariableRef { scope: VariableScope::Resource,  path: vec![attr_sensitivity] }),
        )),
    )).expect("upsert conditional C1");

    // C2: department == "dept-0"
    graph.upsert_policy(Policy::new(
        policy_uuid("conditional-dept", 0),
        "conditional-dept".into(),
        None,
        PolicyType::Permit,
        PolicyTarget::EntityType(user_type),
        PolicyTarget::EntityType(file_type),
        vec![read_id],
        vec![],
        Some(Condition::Eq(
            Operand::Variable(VariableRef { scope: VariableScope::Principal, path: vec![attr_department] }),
            Operand::String("dept-0".into()),
        )),
    )).expect("upsert conditional C2");

    // C3: conditional forbid when sensitivity == 4
    graph.upsert_policy(Policy::new(
        policy_uuid("conditional-sensitivity-forbid", 0),
        "conditional-sensitivity-forbid".into(),
        None,
        PolicyType::Forbid,
        PolicyTarget::EntityType(user_type),
        PolicyTarget::EntityType(file_type),
        vec![read_id],
        vec![],
        Some(Condition::Eq(
            Operand::Variable(VariableRef { scope: VariableScope::Resource, path: vec![attr_sensitivity] }),
            Operand::Integer(4),
        )),
    )).expect("upsert conditional C3");

    // ------------------------------------------------------------------
    // Build snapshot
    // ------------------------------------------------------------------

    let snapshot = SnapshotBuilder::build(&graph).expect("build snapshot");

    // Resolve indices for the returned fixtures.
    let uuid_to_index = &snapshot.uuid_to_index;

    let permitted_principal = *uuid_to_index.get(&permitted_user_id)
        .expect("permitted user not in snapshot");
    let denied_principal = *uuid_to_index.get(&denied_user_id)
        .expect("denied user not in snapshot");
    let resource = *uuid_to_index.get(&specific_file_id)
        .expect("specific file not in snapshot");
    let action = *uuid_to_index.get(&read_id)
        .expect("read action not in snapshot");

    let fixtures = BenchFixtures {
        permitted_principal,
        denied_principal,
        resource,
        action,
        file_type,
    };

    (snapshot, fixtures)
}
