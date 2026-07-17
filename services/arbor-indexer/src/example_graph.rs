/// Builds a small but realistic example graph for development and testing.
///
/// Schema:
///   Types: User(1), Group(2), File(3), Folder(4)
///   Actions: read, write, delete
///
///   Entities:
///     alice (User), bob (User)
///     admins (Group), alice is a member (parent = admins)
///     reports/ (Folder), report.pdf (File, parent = reports/)
///
///   Policies:
///     - permit admins (and descendants) to read/write any File
///     - permit bob to read report.pdf specifically
///     - forbid everyone from deleting any File
use arbor_graph_core::graph::Graph;
use arbor_types::{Action, EntityInput, Policy, PolicyTarget, PolicyType};
use uuid::Uuid;

pub fn build() -> Graph {
    let mut graph = Graph::new();

    // --- stable UUIDs (v5 from a fixed namespace so reruns are deterministic) ---
    let ns = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
    let id = |name: &str| Uuid::new_v5(&ns, name.as_bytes());

    let alice_id      = id("alice");
    let bob_id        = id("bob");
    let admins_id     = id("admins");
    let reports_id    = id("reports/");
    let report_pdf_id = id("report.pdf");

    let read_id   = id("action:read");
    let write_id  = id("action:write");
    let delete_id = id("action:delete");

    let policy_admins_rw_id   = id("policy:admins-rw-files");
    let policy_bob_read_id    = id("policy:bob-read-report");
    let policy_forbid_del_id  = id("policy:forbid-delete-files");

    // --- entities (type names resolved automatically) ---
    graph.upsert_entity_from_input(EntityInput { id: admins_id, name: "admins".into(), type_name: "Group".into(), parents: vec![], attributes: vec![] })
        .expect("upsert admins");
    // alice is a member of admins (admins is her parent group)
    graph.upsert_entity_from_input(EntityInput { id: alice_id, name: "alice".into(), type_name: "User".into(), parents: vec![admins_id], attributes: vec![] })
        .expect("upsert alice");
    graph.upsert_entity_from_input(EntityInput { id: bob_id, name: "bob".into(), type_name: "User".into(), parents: vec![], attributes: vec![] })
        .expect("upsert bob");
    graph.upsert_entity_from_input(EntityInput { id: reports_id, name: "reports/".into(), type_name: "Folder".into(), parents: vec![], attributes: vec![] })
        .expect("upsert reports/");
    // report.pdf lives inside reports/
    graph.upsert_entity_from_input(EntityInput { id: report_pdf_id, name: "report.pdf".into(), type_name: "File".into(), parents: vec![reports_id], attributes: vec![] })
        .expect("upsert report.pdf");

    // Resolve the "File" type id (already registered above via upsert_entity_from_input)
    let file_type_id = graph.get_or_create_entity_type_id("File");

    // --- actions ---
    graph.add_action(Action { id: read_id,   name: "read".into(),   entity_type_id: file_type_id, description: None })
        .expect("add read");
    graph.add_action(Action { id: write_id,  name: "write".into(),  entity_type_id: file_type_id, description: None })
        .expect("add write");
    graph.add_action(Action { id: delete_id, name: "delete".into(), entity_type_id: file_type_id, description: None })
        .expect("add delete");

    // --- policies ---

    // admins group (and all descendants, i.e. alice) can read+write any File
    graph.upsert_policy(Policy::new(
        policy_admins_rw_id,
        "admins-rw-files".into(),
        None,
        PolicyType::Permit,
        PolicyTarget::EntityWithDescendants(admins_id),
        PolicyTarget::EntityType(file_type_id),
        vec![read_id, write_id],
        vec![],
        None,
    )).expect("upsert admins-rw policy");

    // bob can read report.pdf specifically
    graph.upsert_policy(Policy::new(
        policy_bob_read_id,
        "bob-read-report".into(),
        None,
        PolicyType::Permit,
        PolicyTarget::Entity(bob_id),
        PolicyTarget::Entity(report_pdf_id),
        vec![read_id],
        vec![],
        None,
    )).expect("upsert bob-read policy");

    // nobody can delete any File
    graph.upsert_policy(Policy::new(
        policy_forbid_del_id,
        "forbid-delete-files".into(),
        None,
        PolicyType::Forbid,
        PolicyTarget::All,
        PolicyTarget::EntityType(file_type_id),
        vec![delete_id],
        vec![],
        None,
    )).expect("upsert forbid-delete policy");

    graph
}
