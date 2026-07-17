//! Generates a synthetic-but-realistic healthcare clinic network as a set of
//! per-entity-type CSVs plus `connectors.yaml` and `data_model.yaml`, matching
//! the connector design in `crates/arbor-connectors`. Unlike
//! `gen_company_dataset.rs`'s tree-shaped org chart, this dataset is a
//! genuine multi-parent DAG: physicians belong to both a clinic *and* a
//! specialty group, and a slice of patients are registered at a second
//! clinic -- exercising the `parent_ids` (`;`-separated) column added to the
//! CSV connector specifically to support this.
//!
//! Entity shape:
//!   Clinic, SpecialtyGroup       -- two independent root types
//!   Physician, Nurse, Technician,
//!     AdminStaff                -- principal side, parented to a Clinic;
//!                                   Physicians additionally parented to a
//!                                   SpecialtyGroup matching their labeled
//!                                   specialty (consistent, not random)
//!   PatientRoster                -- one per clinic, deliberately *not* a
//!                                   child of that Clinic (kept as a root),
//!                                   so a clinic's own descendant closure
//!                                   covers only its staff/rooms, never its
//!                                   patients
//!   Patient                     -- parented to one roster, plus a second
//!                                   roster for the multi-clinic slice
//!   RoomEquipment                -- parented to a Clinic
//!
//! Staffing is proportional to each clinic's patient count (a fixed
//! patients-per-physician ratio, then nurse/tech/admin as multiples of
//! physician count) rather than independently randomized per entity --
//! headcount follows patient load the way a real clinic's would.
//!
//! Policy shapes (HIPAA-inspired access patterns, not a compliance claim --
//! this is synthetic data):
//!   - Per clinic: physicians/nurses/techs read+write their own clinic's
//!     patient roster (EntityWithDescendants on both sides).
//!   - Per specialty group: physicians in that group can read chart+labs for
//!     any patient (EntityWithDescendants principal, EntityType resource) --
//!     the policy this dataset's multi-parent DAG exists to exercise.
//!   - Admin staff read billing info only, clinic-wide (Treatment/Payment/
//!     Operations boundary -- a distinct action, not full chart access).
//!   - Technicians manage rooms/equipment, clinic-wide.
//!   - Break-glass: a "restricted" slice of patients gets an explicit
//!     per-patient Forbid on chart reads, overriding the broader clinic
//!     permit above it (forbid-overrides-permit).
//!   - Referrals: a slice of patients gets an explicit narrow Permit for one
//!     named physician outside their home clinic, scoped to labs+imaging
//!     only, not the full chart.
//!
//! Usage: gen_healthcare_dataset --patients N [--clinics C] [--out-dir DIR]
//!
//! Run the indexer against the result with:
//!   ARBOR_CONFIG_DIR=<out-dir> cargo run -p arbor-indexer

use std::env;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::PathBuf;

use rand::rngs::StdRng;
use rand::seq::IndexedRandom;
use rand::SeedableRng;
use uuid::Uuid;

const NAMESPACE: Uuid = Uuid::from_u128(0x1b0d2a4e_5f3c_4b7a_9e6d_2c8f7a0b3d11);

const PATIENTS_PER_PHYSICIAN: usize = 150;
const NURSES_PER_PHYSICIAN: usize = 3;
const TECHS_PER_PHYSICIAN: usize = 1;
const ADMIN_PER_PHYSICIANS: usize = 2; // 1 admin per 2 physicians
const ROOMS_PER_CLINIC: usize = 200;

const SPECIALTIES: &[&str] = &[
    "Cardiology", "Oncology", "Pediatrics", "Orthopedics", "Neurology",
    "Dermatology", "Psychiatry", "Endocrinology", "Gastroenterology",
    "Pulmonology", "Rheumatology", "Family Medicine",
];

const CLINIC_CITIES: &[&str] = &[
    "Ashford", "Brookhaven", "Cedar Falls", "Dunmore", "Elmridge", "Fairview",
    "Greenwood", "Harborview", "Ironwood", "Jasper", "Kingsley", "Lakeview",
    "Millbrook", "Northgate", "Oakhurst", "Pinecrest", "Queensbury",
    "Riverside", "Stonebridge", "Thornfield",
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

const ROOM_KINDS: &[&str] = &["Exam Room", "Operating Room", "Imaging Suite", "Infusion Bay", "Recovery Bay"];
const EQUIPMENT_KINDS: &[&str] = &["MRI Scanner", "Ultrasound Unit", "Ventilator", "Infusion Pump", "X-Ray Unit"];

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

/// One row in an entity CSV: `(id, name, parents)`. Always written through a
/// single `parent_ids` (`;`-separated) column regardless of how many parents
/// a row actually has -- 0, 1, or many are all just list lengths.
type EntityRow = (Uuid, String, Vec<Uuid>);

/// One row in `actions.csv`: `(name, entity_type)`. No `id` column -- the
/// connector derives the UUID from `name` itself, the same way
/// `policies.csv`'s `actions` column resolves the names it lists.
type ActionRow = (&'static str, &'static str);

/// One row in `action_sets.csv`: `(name, member action names)`.
type ActionSetRow = (&'static str, Vec<&'static str>);

/// One row in `patients.csv`: `(id, name, parents, consents_to_specialist_sharing)`.
/// Patients are the only entity type carrying an attribute in this dataset --
/// the ABAC gate on the specialty-consult policy below reads it back.
type PatientRow = (Uuid, String, Vec<Uuid>, bool);

struct PolicyRow {
    id: Uuid,
    name: String,
    policy_type: &'static str,
    principal_type: &'static str,
    principal_id: String,
    resource_type: &'static str,
    resource_id: String,
    /// Action *names*, not UUIDs -- resolved by the connector the same way
    /// `actions.csv` rows are.
    actions: Vec<&'static str>,
    action_sets: Vec<&'static str>,
    /// Free-text ABAC condition, parsed by `condition_parser` at ingestion.
    condition: Option<String>,
}

fn main() {
    let mut patients_total: usize = 90_000;
    let mut num_clinics: usize = 20;
    let mut restricted_pct: f64 = 3.0;
    let mut multi_clinic_pct: f64 = 3.0;
    let mut referrals: usize = 1_800;
    let mut out_dir = PathBuf::from("data/healthcare");

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--patients" => patients_total = args.next().expect("--patients needs a value").parse().expect("--patients must be a positive integer"),
            "--clinics" => num_clinics = args.next().expect("--clinics needs a value").parse().expect("--clinics must be a positive integer"),
            "--restricted-pct" => restricted_pct = args.next().expect("--restricted-pct needs a value").parse().expect("--restricted-pct must be a number"),
            "--multi-clinic-pct" => multi_clinic_pct = args.next().expect("--multi-clinic-pct needs a value").parse().expect("--multi-clinic-pct must be a number"),
            "--referrals" => referrals = args.next().expect("--referrals needs a value").parse().expect("--referrals must be a positive integer"),
            "--out-dir" => out_dir = PathBuf::from(args.next().expect("--out-dir needs a value")),
            other => panic!("unrecognized argument: {other}"),
        }
    }
    num_clinics = num_clinics.max(1);

    let mut rng = StdRng::seed_from_u64(42);

    let mut clinics: Vec<EntityRow> = Vec::new();
    let mut specialty_groups: Vec<EntityRow> = Vec::new();
    let mut physicians: Vec<EntityRow> = Vec::new();
    let mut nurses: Vec<EntityRow> = Vec::new();
    let mut technicians: Vec<EntityRow> = Vec::new();
    let mut admin_staff: Vec<EntityRow> = Vec::new();
    let mut rosters: Vec<EntityRow> = Vec::new();
    let mut patients: Vec<PatientRow> = Vec::new();
    let mut rooms: Vec<EntityRow> = Vec::new();
    let mut policies: Vec<PolicyRow> = Vec::new();

    let actions: Vec<ActionRow> = vec![
        ("read_chart", "Patient"),
        ("write_chart", "Patient"),
        ("read_labs", "Patient"),
        ("read_imaging", "Patient"),
        ("read_billing", "Patient"),
        ("read", "RoomEquipment"),
        ("write", "RoomEquipment"),
    ];
    // A named bundle referrals grant as a unit, rather than listing its two
    // member actions on every referral row -- the "role-based access" use
    // `Policy.action_sets` (and this dataset) exists to showcase.
    let action_sets: Vec<ActionSetRow> = vec![("ConsultAccess", vec!["read_labs", "read_imaging"])];

    // Specialty groups exist independently of clinics -- a physician's
    // specialty membership cuts across the clinic hierarchy, which is
    // exactly the multi-parent case the CSV connector's `parent_ids` column
    // was added for.
    let num_groups = SPECIALTIES.len();
    let group_ids: Vec<Uuid> = SPECIALTIES
        .iter()
        .map(|s| {
            let gid = id(&format!("group:{s}"));
            specialty_groups.push((gid, format!("{s} Group"), vec![]));
            gid
        })
        .collect();
    // One specialty consult policy per group: any physician in the group can
    // read chart+labs for any patient, regardless of clinic -- this is the
    // policy whose principal side (`EntityWithDescendants(group)`) only
    // resolves correctly because physicians can have the group as a *second*
    // parent alongside their clinic.
    for (i, gid) in group_ids.iter().enumerate() {
        policies.push(PolicyRow {
            id: id(&format!("policy:specialty-consult:{i}")),
            name: format!("{}-specialty-consult-access", SPECIALTIES[i]),
            policy_type: "permit",
            principal_type: "entity_with_descendants",
            principal_id: gid.to_string(),
            resource_type: "entity_type",
            resource_id: "Patient".into(),
            actions: vec!["read_chart", "read_labs"],
            action_sets: vec![],
            // ABAC gate: broad EntityType(Patient) reach is narrowed to only
            // patients who've consented to specialist sharing -- the targeting
            // says "any patient," the condition says "only if they said yes."
            condition: Some("resource.consent_flags.share_with_specialists == true".to_string()),
        });
    }

    // Round-robin patients across clinics (same remainder-distribution
    // pattern as gen_company_dataset.rs) so clinic sizes are as even as
    // `patients_total` allows, then derive every other headcount from that.
    let mut patient_index = 0usize;
    let mut clinic_ids: Vec<Uuid> = Vec::new();
    let mut roster_ids: Vec<Uuid> = Vec::new();
    let mut all_physicians: Vec<(Uuid, usize)> = Vec::new(); // (id, group index)

    for clinic_i in 0..num_clinics {
        let city = CLINIC_CITIES[clinic_i % CLINIC_CITIES.len()];
        let clinic_name = format!("{city} Clinic");
        let clinic_id = id(&format!("clinic:{clinic_i}"));
        clinics.push((clinic_id, clinic_name.clone(), vec![]));
        clinic_ids.push(clinic_id);

        // Deliberately not parented to the clinic -- see module docs. Kept
        // as its own root so a clinic's descendant closure never includes
        // patients, only staff and rooms.
        let roster_id = id(&format!("roster:{clinic_i}"));
        rosters.push((roster_id, format!("{clinic_name} Patient Roster"), vec![]));
        roster_ids.push(roster_id);

        let clinic_patient_count = patients_total / num_clinics
            + if clinic_i < patients_total % num_clinics { 1 } else { 0 };

        for _ in 0..clinic_patient_count {
            let first = FIRST_NAMES.choose(&mut rng).unwrap();
            let last = LAST_NAMES.choose(&mut rng).unwrap();
            let patient_id = id(&format!("patient:{patient_index}"));
            // Deterministic, not per-row RNG (matching how staffing ratios
            // are derived rather than randomized): roughly 6 in 7 patients
            // consent to specialty-group sharing.
            let consents_to_sharing = patient_index % 7 != 0;
            patient_index += 1;
            patients.push((patient_id, format!("{first} {last}"), vec![roster_id], consents_to_sharing));
        }

        // Headcount follows patient load, not independent per-entity
        // randomness: a bigger clinic gets proportionally more of every role.
        let physician_count = clinic_patient_count.div_ceil(PATIENTS_PER_PHYSICIAN).max(1);
        let nurse_count = physician_count * NURSES_PER_PHYSICIAN;
        let tech_count = physician_count * TECHS_PER_PHYSICIAN;
        let admin_count = physician_count.div_ceil(ADMIN_PER_PHYSICIANS).max(1);

        for p in 0..physician_count {
            // Specialty assignment is deterministic (round-robin over the
            // fixed specialty list, offset by clinic), not per-physician
            // random -- so every specialty group's membership is evenly and
            // predictably drawn from across all clinics, and the physician's
            // labeled specialty always matches the group they're parented to.
            let group_idx = (clinic_i + p) % num_groups;
            let specialty = SPECIALTIES[group_idx];
            let first = FIRST_NAMES.choose(&mut rng).unwrap();
            let last = LAST_NAMES.choose(&mut rng).unwrap();
            let phys_id = id(&format!("physician:{clinic_i}:{p}"));
            physicians.push((
                phys_id,
                format!("Dr. {first} {last} ({specialty})"),
                vec![clinic_id, group_ids[group_idx]],
            ));
            all_physicians.push((phys_id, group_idx));
        }

        for n in 0..nurse_count {
            let first = FIRST_NAMES.choose(&mut rng).unwrap();
            let last = LAST_NAMES.choose(&mut rng).unwrap();
            nurses.push((id(&format!("nurse:{clinic_i}:{n}")), format!("{first} {last}, RN"), vec![clinic_id]));
        }
        for t in 0..tech_count {
            let first = FIRST_NAMES.choose(&mut rng).unwrap();
            let last = LAST_NAMES.choose(&mut rng).unwrap();
            technicians.push((id(&format!("tech:{clinic_i}:{t}")), format!("{first} {last}, Tech"), vec![clinic_id]));
        }
        for a in 0..admin_count {
            let first = FIRST_NAMES.choose(&mut rng).unwrap();
            let last = LAST_NAMES.choose(&mut rng).unwrap();
            // Not parented to the clinic: admin staff's only grant is the
            // global billing-only policy below. Parenting them under the
            // clinic like the clinical roles would make them a descendant
            // swept into the clinic-wide chart read/write policy too,
            // defeating the Treatment/Payment/Operations boundary that
            // policy exists to enforce.
            admin_staff.push((id(&format!("admin:{clinic_i}:{a}")), format!("{first} {last} ({clinic_name})"), vec![]));
        }

        for r in 0..ROOMS_PER_CLINIC {
            let kind = if r % 3 == 0 { EQUIPMENT_KINDS.choose(&mut rng).unwrap() } else { ROOM_KINDS.choose(&mut rng).unwrap() };
            rooms.push((id(&format!("room:{clinic_i}:{r}")), format!("{clinic_name} {kind} {r}"), vec![clinic_id]));
        }

        // Per-clinic access: physicians, nurses, and technicians at this
        // clinic can read+write this clinic's own patient roster.
        // EntityWithDescendants(clinic_id) only reaches staff and rooms
        // (never patients, since the roster isn't a graph child of the
        // clinic), and EntityWithDescendants(roster_id) reaches every
        // patient registered there, including multi-clinic patients added
        // below via a second roster parent.
        policies.push(PolicyRow {
            id: id(&format!("policy:clinic-staff-roster:{clinic_i}")),
            name: format!("{clinic_name}-staff-read-write-own-roster"),
            policy_type: "permit",
            principal_type: "entity_with_descendants",
            principal_id: clinic_id.to_string(),
            resource_type: "entity_with_descendants",
            resource_id: roster_id.to_string(),
            actions: vec!["read_chart", "write_chart"],
            action_sets: vec![],
            condition: None,
        });
    }

    // Admin staff see billing only, clinic-wide -- a distinct action from
    // chart access, modeling the Treatment/Payment/Operations boundary.
    policies.push(PolicyRow {
        id: id("policy:admin-billing-access"),
        name: "admin-staff-read-billing".into(),
        policy_type: "permit",
        principal_type: "entity_type",
        principal_id: "AdminStaff".into(),
        resource_type: "entity_type",
        resource_id: "Patient".into(),
        actions: vec!["read_billing"],
        action_sets: vec![],
        condition: None,
    });

    // Technicians manage rooms/equipment, clinic-wide.
    policies.push(PolicyRow {
        id: id("policy:tech-room-access"),
        name: "technicians-manage-rooms-equipment".into(),
        policy_type: "permit",
        principal_type: "entity_type",
        principal_id: "Technician".into(),
        resource_type: "entity_type",
        resource_id: "RoomEquipment".into(),
        actions: vec!["read", "write"],
        action_sets: vec![],
        condition: None,
    });

    // Multi-clinic patients: a slice gets a second roster parent (a
    // different clinic than their first), producing a true multi-parent
    // patient -- the resource-side DAG stress case, paired with physicians'
    // multi-parent case above.
    let multi_clinic_count = ((patients.len() as f64) * (multi_clinic_pct / 100.0)) as usize;
    for i in 0..multi_clinic_count.min(patients.len()) {
        let idx = (i * 2_654_435_761usize) % patients.len(); // deterministic spread, not sequential
        let home_roster = patients[idx].2[0];
        let home_clinic = roster_ids.iter().position(|r| *r == home_roster).unwrap_or(0);
        let second_clinic = (home_clinic + 1) % num_clinics;
        let second_roster = roster_ids[second_clinic];
        if !patients[idx].2.contains(&second_roster) {
            patients[idx].2.push(second_roster);
        }
    }

    // Break-glass: a slice of patients is flagged restricted via an
    // explicit per-patient Forbid on chart reads. This overrides the
    // broader clinic-wide permit above for that one patient specifically --
    // forbid-overrides-permit, the textbook case this dataset exists to
    // exercise. A real system would pair this with a break-glass-with-reason
    // exception; omitted here since the CSV connector doesn't carry
    // conditions yet.
    let restricted_count = ((patients.len() as f64) * (restricted_pct / 100.0)) as usize;
    for i in 0..restricted_count.min(patients.len()) {
        let idx = (i * 40_503usize) % patients.len();
        let (patient_id, _, _, _) = &patients[idx];
        policies.push(PolicyRow {
            id: id(&format!("policy:restricted-forbid:{i}")),
            name: format!("restricted-patient-{i}-forbid-chart-read"),
            policy_type: "forbid",
            principal_type: "entity_type",
            principal_id: "Physician".into(),
            resource_type: "entity",
            resource_id: patient_id.to_string(),
            actions: vec!["read_chart"],
            action_sets: vec![],
            condition: None,
        });
    }

    // Referrals: a slice of patients gets one named physician outside their
    // home clinic an explicit, narrow grant -- the ConsultAccess bundle
    // (labs+imaging only, not the full chart) -- modeling a one-off consult
    // rather than ongoing care, and referenced as a set rather than as two
    // loose actions repeated on every row.
    let referral_count = referrals.min(patients.len());
    for i in 0..referral_count {
        let p_idx = (i * 2_246_822_519usize) % patients.len();
        let phys_idx = (i * 3_266_489_917usize) % all_physicians.len();
        let (patient_id, _, _, _) = &patients[p_idx];
        let (phys_id, _) = &all_physicians[phys_idx];
        policies.push(PolicyRow {
            id: id(&format!("policy:referral:{i}")),
            name: format!("referral-{i}-consult-access"),
            policy_type: "permit",
            principal_type: "entity",
            principal_id: phys_id.to_string(),
            resource_type: "entity",
            resource_id: patient_id.to_string(),
            actions: vec![],
            action_sets: vec!["ConsultAccess"],
            condition: None,
        });
    }

    fs::create_dir_all(&out_dir).expect("failed to create out-dir");

    write_entities_csv(&out_dir.join("clinics.csv"), "clinic_id,clinic_name,parent_ids", &clinics);
    write_entities_csv(&out_dir.join("specialty_groups.csv"), "group_id,group_name,parent_ids", &specialty_groups);
    write_entities_csv(&out_dir.join("rosters.csv"), "roster_id,roster_name,parent_ids", &rosters);
    write_entities_csv(&out_dir.join("physicians.csv"), "physician_id,physician_name,parent_ids", &physicians);
    write_entities_csv(&out_dir.join("nurses.csv"), "nurse_id,nurse_name,parent_ids", &nurses);
    write_entities_csv(&out_dir.join("technicians.csv"), "tech_id,tech_name,parent_ids", &technicians);
    write_entities_csv(&out_dir.join("admin_staff.csv"), "admin_id,admin_name,parent_ids", &admin_staff);
    write_patients_csv(&out_dir.join("patients.csv"), &patients);
    write_entities_csv(&out_dir.join("rooms_equipment.csv"), "room_id,room_name,parent_ids", &rooms);
    write_actions_csv(&out_dir.join("actions.csv"), &actions);
    write_action_sets_csv(&out_dir.join("action_sets.csv"), &action_sets);
    write_policies_csv(&out_dir.join("policies.csv"), &policies);
    write_connectors_yaml(&out_dir.join("connectors.yaml"));
    write_data_model_yaml(&out_dir.join("data_model.yaml"));

    let counts = [
        ("Clinic", clinics.len()),
        ("SpecialtyGroup", specialty_groups.len()),
        ("PatientRoster", rosters.len()),
        ("Physician", physicians.len()),
        ("Nurse", nurses.len()),
        ("Technician", technicians.len()),
        ("AdminStaff", admin_staff.len()),
        ("Patient", patients.len()),
        ("RoomEquipment", rooms.len()),
    ];
    let total: usize = counts.iter().map(|(_, n)| n).sum();
    println!("Wrote {total} entities, {} policies to {}", policies.len(), out_dir.display());
    for (type_name, count) in counts {
        println!("  {type_name:<15} {count}");
    }
    println!("  multi-clinic patients: {}", multi_clinic_count.min(patients.len()));
    println!("  restricted (forbid) patients: {}", restricted_count.min(patients.len()));
    println!("  referrals: {referral_count}");
    println!("\nRun the indexer against this dataset with:");
    println!("  ARBOR_CONFIG_DIR={} cargo run -p arbor-indexer", out_dir.display());
}

fn write_entities_csv(path: &PathBuf, header: &str, rows: &[EntityRow]) {
    let mut w = BufWriter::new(File::create(path).expect("failed to create csv"));
    writeln!(w, "{header}").unwrap();
    for (id, name, parents) in rows {
        let parents = parents.iter().map(Uuid::to_string).collect::<Vec<_>>().join(";");
        writeln!(w, "{id},{},{parents}", csv_field(name)).unwrap();
    }
}

fn write_patients_csv(path: &PathBuf, rows: &[PatientRow]) {
    let mut w = BufWriter::new(File::create(path).expect("failed to create patients.csv"));
    writeln!(w, "patient_id,patient_name,parent_ids,consents_to_specialist_sharing").unwrap();
    for (id, name, parents, consents) in rows {
        let parents = parents.iter().map(Uuid::to_string).collect::<Vec<_>>().join(";");
        writeln!(w, "{id},{},{parents},{consents}", csv_field(name)).unwrap();
    }
}

fn write_actions_csv(path: &PathBuf, rows: &[ActionRow]) {
    let mut w = BufWriter::new(File::create(path).expect("failed to create actions.csv"));
    writeln!(w, "action_name,scoped_type").unwrap();
    for (name, scoped_type) in rows {
        writeln!(w, "{name},{scoped_type}").unwrap();
    }
}

fn write_action_sets_csv(path: &PathBuf, rows: &[ActionSetRow]) {
    let mut w = BufWriter::new(File::create(path).expect("failed to create action_sets.csv"));
    writeln!(w, "set_name,member_actions").unwrap();
    for (name, members) in rows {
        writeln!(w, "{name},{}", members.join(";")).unwrap();
    }
}

fn write_policies_csv(path: &PathBuf, rows: &[PolicyRow]) {
    let mut w = BufWriter::new(File::create(path).expect("failed to create policies.csv"));
    writeln!(w, "policy_id,policy_name,ptype,principal_kind,principal,resource_kind,resource,action_names,set_names,condition_text").unwrap();
    for row in rows {
        writeln!(
            w,
            "{},{},{},{},{},{},{},{},{},{}",
            row.id, csv_field(&row.name), row.policy_type,
            row.principal_type, row.principal_id,
            row.resource_type, row.resource_id,
            row.actions.join(";"),
            row.action_sets.join(";"),
            csv_field(row.condition.as_deref().unwrap_or("")),
        ).unwrap();
    }
}

fn write_connectors_yaml(path: &PathBuf) {
    let mut w = BufWriter::new(File::create(path).expect("failed to create connectors.yaml"));
    write!(w, "{}", r#"# Connection info only -- for csv connectors, just where the file lives.
# What each file *means* (entity type, column mapping) is in data_model.yaml.
connectors:
  clinics_csv:
    type: csv
    file: clinics.csv
  specialty_groups_csv:
    type: csv
    file: specialty_groups.csv
  rosters_csv:
    type: csv
    file: rosters.csv
  physicians_csv:
    type: csv
    file: physicians.csv
  nurses_csv:
    type: csv
    file: nurses.csv
  technicians_csv:
    type: csv
    file: technicians.csv
  admin_staff_csv:
    type: csv
    file: admin_staff.csv
  patients_csv:
    type: csv
    file: patients.csv
  rooms_equipment_csv:
    type: csv
    file: rooms_equipment.csv
  actions_csv:
    type: csv
    file: actions.csv
  action_sets_csv:
    type: csv
    file: action_sets.csv
  policies_csv:
    type: csv
    file: policies.csv
"#).unwrap();
}

fn write_data_model_yaml(path: &PathBuf) {
    let mut w = BufWriter::new(File::create(path).expect("failed to create data_model.yaml"));
    write!(w, "{}", r#"entity_types:
  - name: Clinic
    connector: clinics_csv
    columns:
      id: clinic_id
      name: clinic_name

  - name: SpecialtyGroup
    connector: specialty_groups_csv
    columns:
      id: group_id
      name: group_name

  - name: PatientRoster
    connector: rosters_csv
    columns:
      id: roster_id
      name: roster_name

  - name: Physician
    connector: physicians_csv
    columns:
      id: physician_id
      name: physician_name
      parent_ids: parent_ids   # clinic + specialty group -- a real multi-parent DAG

  - name: Nurse
    connector: nurses_csv
    columns:
      id: nurse_id
      name: nurse_name
      parent_ids: parent_ids

  - name: Technician
    connector: technicians_csv
    columns:
      id: tech_id
      name: tech_name
      parent_ids: parent_ids

  - name: AdminStaff
    connector: admin_staff_csv
    columns:
      id: admin_id
      name: admin_name
      parent_ids: parent_ids

  - name: Patient
    connector: patients_csv
    columns:
      id: patient_id
      name: patient_name
      parent_ids: parent_ids   # one roster normally, two for multi-clinic patients
      attributes:
        - path: consent_flags.share_with_specialists
          column: consents_to_specialist_sharing
          value_type: bool

  - name: RoomEquipment
    connector: rooms_equipment_csv
    columns:
      id: room_id
      name: room_name
      parent_ids: parent_ids

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
      actions: action_names       # ';'-separated action *names*, not UUIDs
      action_sets: set_names      # ';'-separated action-set names, optional
      condition: condition_text  # optional free-text ABAC condition

actions:
  - connector: actions_csv
    columns:
      name: action_name
      entity_type: scoped_type

action_sets:
  - connector: action_sets_csv
    columns:
      name: set_name
      actions: member_actions
"#).unwrap();
}
