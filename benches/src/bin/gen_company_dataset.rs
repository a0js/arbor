//! Generates a synthetic-but-realistic company org chart + file tree as a set
//! of per-entity-type CSVs plus a `connectors.yaml` (file locations) and
//! `data_model.yaml` (entity type + policy column mappings), matching the
//! connector design in `crates/arbor-connectors`. Each output CSV uses
//! header names an external export would plausibly use (not Arbor's
//! internal field names), to actually exercise the column mapping rather
//! than coincidentally matching it.
//!
//! Shape: Company -> Departments -> Teams -> Employees, plus a parallel
//! Company -> Departments -> Teams folder tree holding Files. One team is
//! designated the "admin team" with elevated file access, and a company-wide
//! "Public" folder is readable by every employee.
//!
//! Deliberately keeps policy count at O(departments) rather than O(entities):
//! one permit per department (scoped via `EntityWithDescendants` on both
//! sides), one for the admin team, one for company-wide Public reads. That's
//! the property being showcased -- entity count scales with `--employees`,
//! policy count does not.
//!
//! Usage: gen_company_dataset --employees N [--departments D] [--out-dir DIR]
//!
//! Run the indexer against the result with:
//!   ARBOR_CONFIG_DIR=<out-dir> cargo run -p arbor-indexer

use std::env;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::PathBuf;

use arbor_types::Action;
use rand::rngs::StdRng;
use rand::seq::IndexedRandom;
use rand::SeedableRng;
use uuid::Uuid;

const NAMESPACE: Uuid = Uuid::from_u128(0x6ba7b810_9dad_11d1_80b4_00c04fd430c8);
const TEAM_SIZE: usize = 7;
const FILES_PER_EMPLOYEE: usize = 3;
const ADMIN_DEPARTMENT: &str = "IT";

const DEPARTMENTS: &[&str] = &[
    "Engineering", "Sales", "Marketing", "Finance", "Human Resources",
    "Legal", "Customer Support", "Product", "Operations", "IT",
];

const FIRST_NAMES: &[&str] = &[
    "James", "Mary", "Robert", "Patricia", "John", "Jennifer", "Michael", "Linda",
    "David", "Elizabeth", "William", "Barbara", "Richard", "Susan", "Joseph", "Jessica",
    "Thomas", "Sarah", "Charles", "Karen", "Priya", "Wei", "Fatima", "Hiroshi",
    "Ingrid", "Mateo", "Amara", "Yuki", "Sofia", "Kwame",
];

const LAST_NAMES: &[&str] = &[
    "Smith", "Johnson", "Williams", "Brown", "Jones", "Garcia", "Miller", "Davis",
    "Rodriguez", "Martinez", "Hernandez", "Lopez", "Gonzalez", "Wilson", "Anderson",
    "Thomas", "Taylor", "Moore", "Jackson", "Martin", "Lee", "Perez", "Thompson",
    "White", "Harris", "Sanchez", "Clark", "Ramirez", "Lewis", "Robinson",
];

const FILE_TOPICS: &[&str] = &[
    "roadmap", "budget", "onboarding", "retro-notes", "design-doc", "status-report",
    "meeting-notes", "proposal", "postmortem", "handbook",
];

const FILE_EXTS: &[&str] = &["pdf", "docx", "xlsx", "md", "pptx"];

fn id(seed: &str) -> Uuid {
    Uuid::new_v5(&NAMESPACE, seed.as_bytes())
}

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// One row in an entity CSV: `(id, name, parent_id)`. `parent_id` is empty
/// for roots -- each row has at most one parent, matching a typical export's
/// single "manager_id" / "parent_id" style column.
type EntityRow = (Uuid, String, Option<Uuid>);

struct PolicyRow {
    id: Uuid,
    name: String,
    policy_type: &'static str,
    principal_type: &'static str,
    principal_id: String,
    resource_type: &'static str,
    resource_id: String,
    actions: Vec<Uuid>,
}

fn main() {
    let mut employees_total: usize = 500;
    let mut num_departments: usize = DEPARTMENTS.len();
    let mut out_dir = PathBuf::from("data");

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--employees" => employees_total = args.next().expect("--employees needs a value").parse().expect("--employees must be a positive integer"),
            "--departments" => num_departments = args.next().expect("--departments needs a value").parse().expect("--departments must be a positive integer"),
            "--out-dir" => out_dir = PathBuf::from(args.next().expect("--out-dir needs a value")),
            other => panic!("unrecognized argument: {other}"),
        }
    }
    num_departments = num_departments.min(DEPARTMENTS.len()).max(1);

    let mut rng = StdRng::seed_from_u64(42);

    let mut companies: Vec<EntityRow> = Vec::new();
    let mut departments: Vec<EntityRow> = Vec::new();
    let mut teams: Vec<EntityRow> = Vec::new();
    let mut employees: Vec<EntityRow> = Vec::new();
    let mut folders: Vec<EntityRow> = Vec::new();
    let mut files: Vec<EntityRow> = Vec::new();
    let mut policies: Vec<PolicyRow> = Vec::new();

    let read_id = Action::hash_action_reference("action:read");
    let write_id = Action::hash_action_reference("action:write");
    let delete_id = Action::hash_action_reference("action:delete");

    let company_id = id("company:acme");
    companies.push((company_id, "Acme Corporation".into(), None));

    let company_folder_id = id("folder:acme-root");
    folders.push((company_folder_id, "Acme Corporation/Files".into(), Some(company_id)));

    let public_folder_id = id("folder:acme-public");
    folders.push((public_folder_id, "Acme Corporation/Files/Public".into(), Some(company_folder_id)));

    let mut admin_team_id: Option<Uuid> = None;

    // Round-robin employees across departments so counts are as even as `employees_total`
    // allows, then split each department's employees into fixed-size teams.
    let mut employee_index = 0usize;

    for dept_i in 0..num_departments {
        let dept_name = DEPARTMENTS[dept_i];
        let dept_id = id(&format!("dept:{dept_name}"));
        departments.push((dept_id, dept_name.into(), Some(company_id)));

        let dept_folder_id = id(&format!("folder:dept:{dept_name}"));
        folders.push((dept_folder_id, format!("Acme Corporation/Files/{dept_name}"), Some(company_folder_id)));

        // One permit per department, scoped to its own subtree on both sides --
        // this is the policy that has to cover every employee/team/file the
        // department will ever contain, without growing as they're added.
        policies.push(PolicyRow {
            id: id(&format!("policy:dept-rw:{dept_name}")),
            name: format!("{dept_name}-read-write-own-files"),
            policy_type: "permit",
            principal_type: "entity_with_descendants",
            principal_id: dept_id.to_string(),
            resource_type: "entity_with_descendants",
            resource_id: dept_folder_id.to_string(),
            actions: vec![read_id, write_id],
        });

        let dept_employee_count = employees_total / num_departments
            + if dept_i < employees_total % num_departments { 1 } else { 0 };
        let num_teams = dept_employee_count.div_ceil(TEAM_SIZE).max(1);

        let mut remaining = dept_employee_count;
        for team_i in 0..num_teams {
            let team_name = format!("{dept_name} Team {}", team_i + 1);
            let team_id = id(&format!("team:{dept_name}:{team_i}"));
            teams.push((team_id, team_name.clone(), Some(dept_id)));

            if dept_name == ADMIN_DEPARTMENT && team_i == 0 {
                admin_team_id = Some(team_id);
            }

            let team_folder_id = id(&format!("folder:team:{dept_name}:{team_i}"));
            folders.push((team_folder_id, format!("Acme Corporation/Files/{dept_name}/{team_name}"), Some(dept_folder_id)));

            let team_size = remaining.min(TEAM_SIZE);
            remaining -= team_size;

            for _ in 0..team_size {
                let first = FIRST_NAMES.choose(&mut rng).unwrap();
                let last = LAST_NAMES.choose(&mut rng).unwrap();
                let emp_id = id(&format!("employee:{employee_index}"));
                employee_index += 1;

                employees.push((emp_id, format!("{first} {last}"), Some(team_id)));

                for f in 0..FILES_PER_EMPLOYEE {
                    let topic = FILE_TOPICS.choose(&mut rng).unwrap();
                    let ext = FILE_EXTS.choose(&mut rng).unwrap();
                    let file_id = id(&format!("file:{employee_index}:{f}"));
                    files.push((file_id, format!("{first}.{last}-{topic}-{f}.{ext}"), Some(team_folder_id)));
                }
            }
        }
    }

    // The admin team can read/write/delete any file, company-wide -- expressed
    // as a single policy regardless of how many people are on that team.
    if let Some(admin_team_id) = admin_team_id {
        policies.push(PolicyRow {
            id: id("policy:admin-team-full-access"),
            name: "admin-team-full-file-access".into(),
            policy_type: "permit",
            principal_type: "entity_with_descendants",
            principal_id: admin_team_id.to_string(),
            resource_type: "entity_type",
            resource_id: "File".into(),
            actions: vec![read_id, write_id, delete_id],
        });
    }

    // Every employee can read the company-wide Public folder, expressed by
    // type rather than by listing employees -- this is the policy that
    // *doesn't* grow at all as headcount scales.
    policies.push(PolicyRow {
        id: id("policy:all-employees-read-public"),
        name: "all-employees-read-public".into(),
        policy_type: "permit",
        principal_type: "entity_type",
        principal_id: "Employee".into(),
        resource_type: "entity_with_descendants",
        resource_id: public_folder_id.to_string(),
        actions: vec![read_id],
    });

    fs::create_dir_all(&out_dir).expect("failed to create out-dir");

    write_entities_csv(&out_dir.join("companies.csv"), "company_id,company_name,parent_company_id", &companies);
    write_entities_csv(&out_dir.join("departments.csv"), "dept_id,dept_name,parent_company_id", &departments);
    write_entities_csv(&out_dir.join("teams.csv"), "team_id,team_name,parent_dept_id", &teams);
    write_entities_csv(&out_dir.join("employees.csv"), "emp_id,full_name,parent_team_id", &employees);
    write_entities_csv(&out_dir.join("folders.csv"), "folder_id,folder_name,parent_id", &folders);
    write_entities_csv(&out_dir.join("files.csv"), "file_id,file_name,parent_folder_id", &files);
    write_policies_csv(&out_dir.join("policies.csv"), &policies);
    write_connectors_yaml(&out_dir.join("connectors.yaml"));
    write_data_model_yaml(&out_dir.join("data_model.yaml"));

    let counts = [
        ("Company", companies.len()),
        ("Department", departments.len()),
        ("Team", teams.len()),
        ("Employee", employees.len()),
        ("Folder", folders.len()),
        ("File", files.len()),
    ];
    let total: usize = counts.iter().map(|(_, n)| n).sum();
    println!("Wrote {total} entities, {} policies to {}", policies.len(), out_dir.display());
    for (type_name, count) in counts {
        println!("  {type_name:<12} {count}");
    }
    println!("\nRun the indexer against this dataset with:");
    println!("  ARBOR_CONFIG_DIR={} cargo run -p arbor-indexer", out_dir.display());
}

fn write_entities_csv(path: &PathBuf, header: &str, rows: &[EntityRow]) {
    let mut w = BufWriter::new(File::create(path).expect("failed to create csv"));
    writeln!(w, "{header}").unwrap();
    for (id, name, parent) in rows {
        let parent = parent.map(|p| p.to_string()).unwrap_or_default();
        writeln!(w, "{id},{},{parent}", csv_field(name)).unwrap();
    }
}

fn write_policies_csv(path: &PathBuf, rows: &[PolicyRow]) {
    let mut w = BufWriter::new(File::create(path).expect("failed to create policies.csv"));
    writeln!(w, "policy_id,policy_name,ptype,principal_kind,principal,resource_kind,resource,action_ids").unwrap();
    for row in rows {
        let actions = row.actions.iter().map(Uuid::to_string).collect::<Vec<_>>().join(";");
        writeln!(
            w,
            "{},{},{},{},{},{},{},{}",
            row.id, csv_field(&row.name), row.policy_type,
            row.principal_type, row.principal_id,
            row.resource_type, row.resource_id,
            actions,
        ).unwrap();
    }
}

fn write_connectors_yaml(path: &PathBuf) {
    let mut w = BufWriter::new(File::create(path).expect("failed to create connectors.yaml"));
    write!(w, "{}", r#"# Connection info only -- for csv connectors, just where the file lives.
# What each file *means* (entity type, column mapping) is in entity_types.yaml.
connectors:
  companies_csv:
    type: csv
    file: companies.csv
  departments_csv:
    type: csv
    file: departments.csv
  teams_csv:
    type: csv
    file: teams.csv
  employees_csv:
    type: csv
    file: employees.csv
  folders_csv:
    type: csv
    file: folders.csv
  files_csv:
    type: csv
    file: files.csv
  policies_csv:
    type: csv
    file: policies.csv
"#).unwrap();
}

fn write_data_model_yaml(path: &PathBuf) {
    let mut w = BufWriter::new(File::create(path).expect("failed to create data_model.yaml"));
    write!(w, "{}", r#"entity_types:
  - name: Company
    connector: companies_csv
    columns:
      id: company_id
      name: company_name

  - name: Department
    connector: departments_csv
    columns:
      id: dept_id
      name: dept_name
      parent_id: parent_company_id

  - name: Team
    connector: teams_csv
    columns:
      id: team_id
      name: team_name
      parent_id: parent_dept_id

  - name: Employee
    connector: employees_csv
    columns:
      id: emp_id
      name: full_name
      parent_id: parent_team_id

  - name: Folder
    connector: folders_csv
    columns:
      id: folder_id
      name: folder_name
      parent_id: parent_id

  - name: File
    connector: files_csv
    columns:
      id: file_id
      name: file_name
      parent_id: parent_folder_id

policies:
  - connector: policies_csv
    columns:
      id: policy_id
      name: policy_name
      policy_type: ptype
      principal_type: principal_kind
      principal_id: principal
      resource_type: resource_kind
      resource_id: resource
      actions: action_ids
"#).unwrap();
}
