use std::collections::HashMap;
use std::path::Path;

use config::{Config, Environment};
use serde::Deserialize;
use uuid::Uuid;

use arbor_types::{ArborError, ArborResult, EntityInput, PolicyInput, PolicyTargetInput, PolicyType};

/// One CSV holds rows of exactly one entity type (e.g. `employees.csv` is all
/// `Employee`s) with a single, optional parent reference per row -- this
/// mirrors how most external systems export a hierarchy (one "manager_id" /
/// "parent_id" style column), not an arbitrary DAG. Column *names* are
/// declared here rather than assumed, since a real export's headers won't
/// match Arbor's internal field names.
#[derive(Debug, Deserialize)]
pub struct CsvEntityColumns {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub parent_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CsvPolicyColumns {
    pub id: String,
    pub name: String,
    pub policy_type: String,
    pub principal_type: String,
    pub principal_id: String,
    pub resource_type: String,
    pub resource_id: String,
    /// A single column holding a `;`-separated list of action UUIDs --
    /// unlike parents, a policy's action set genuinely is multi-valued.
    pub actions: String,
}

/// A named connection. Carries only *where the data lives* -- for `postgres`
/// this is also where credentials will live once implemented. Column mapping
/// / query text lives in `entity_types.yaml`, which references connectors by
/// name -- the same split the SQL-query design uses, so a CSV connector can
/// later be swapped for a `postgres` one without changing that shape.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConnectorConfig {
    Csv {
        file: String,
    },
    Postgres {
        host: String,
        port: u16,
        user: String,
        password: String,
        database: String,
    },
}

#[derive(Debug, Deserialize)]
pub struct ConnectorConfigFile {
    pub connectors: HashMap<String, ConnectorConfig>,
}

/// Loads `<config_dir>/connectors.yaml`, optionally overridden by
/// `<config_dir>/connectors.local.yaml` and `ARBOR__CONNECTORS__*` env vars.
pub fn load_connector_config(config_dir: impl AsRef<Path>) -> Result<ConnectorConfigFile, config::ConfigError> {
    let dir = config_dir.as_ref();
    Config::builder()
        .add_source(config::File::from(dir.join("connectors.yaml")))
        .add_source(config::File::from(dir.join("connectors.local.yaml")).required(false))
        .add_source(Environment::with_prefix("ARBOR").separator("__"))
        .build()?
        .try_deserialize()
}

/// One entity type's source: which connector to read it from, and (for CSV
/// connectors) the column mapping. One entry per entity type.
#[derive(Debug, Deserialize)]
pub struct EntityTypeEntry {
    pub name: String,
    pub connector: String,
    pub columns: CsvEntityColumns,
}

/// One policy source: which connector to read it from, and its column mapping.
#[derive(Debug, Deserialize)]
pub struct PolicySourceEntry {
    pub connector: String,
    pub columns: CsvPolicyColumns,
}

/// Both entity types and policies are "data model" config, as opposed to
/// `connectors.yaml`'s connection info -- kept in one file so the name
/// doesn't imply it's entity-types-only.
#[derive(Debug, Deserialize, Default)]
pub struct DataModelConfigFile {
    #[serde(default)]
    pub entity_types: Vec<EntityTypeEntry>,
    #[serde(default)]
    pub policies: Vec<PolicySourceEntry>,
}

/// Loads `<config_dir>/data_model.yaml`, optionally overridden by
/// `<config_dir>/data_model.local.yaml` and `ARBOR__*` env vars.
pub fn load_data_model_config(config_dir: impl AsRef<Path>) -> Result<DataModelConfigFile, config::ConfigError> {
    let dir = config_dir.as_ref();
    Config::builder()
        .add_source(config::File::from(dir.join("data_model.yaml")))
        .add_source(config::File::from(dir.join("data_model.local.yaml")).required(false))
        .add_source(Environment::with_prefix("ARBOR").separator("__"))
        .build()?
        .try_deserialize()
}

fn resolve_csv_file<'a>(connectors: &'a ConnectorConfigFile, name: &str) -> ArborResult<&'a str> {
    match connectors.connectors.get(name) {
        Some(ConnectorConfig::Csv { file }) => Ok(file.as_str()),
        Some(ConnectorConfig::Postgres { .. }) => Err(ArborError::ConversionError(format!(
            "connector {name:?} is a postgres connector, but data_model.yaml column mapping requires a csv connector"
        ))),
        None => Err(ArborError::ConversionError(format!(
            "data_model.yaml references unknown connector {name:?}"
        ))),
    }
}

/// Reads every entity and policy named in `data_model`, resolving each
/// entry's `connector` against `connectors` and joining its `file` against
/// `base_dir`. Validates connector references up front (before opening any
/// files) so a typo'd connector name fails fast with the entry that named it.
pub fn load_all(
    connectors: &ConnectorConfigFile,
    data_model: &DataModelConfigFile,
    base_dir: impl AsRef<Path>,
) -> ArborResult<(Vec<EntityInput>, Vec<PolicyInput>)> {
    let base = base_dir.as_ref();

    for entry in &data_model.entity_types {
        resolve_csv_file(connectors, &entry.connector).map_err(|e| {
            ArborError::ConversionError(format!("entity_type {:?}: {e}", entry.name))
        })?;
    }
    for entry in &data_model.policies {
        resolve_csv_file(connectors, &entry.connector).map_err(|e| {
            ArborError::ConversionError(format!("policy source (connector {:?}): {e}", entry.connector))
        })?;
    }

    let mut entities = Vec::new();
    for entry in &data_model.entity_types {
        let file = resolve_csv_file(connectors, &entry.connector)?;
        entities.extend(
            read_entities_csv(base.join(file), &entry.name, &entry.columns).map_err(|e| {
                ArborError::ConversionError(format!(
                    "entity_type {:?} (connector {:?}): {e}",
                    entry.name, entry.connector
                ))
            })?,
        );
    }

    let mut policies = Vec::new();
    for entry in &data_model.policies {
        let file = resolve_csv_file(connectors, &entry.connector)?;
        policies.extend(read_policies_csv(base.join(file), &entry.columns).map_err(|e| {
            ArborError::ConversionError(format!("policy source (connector {:?}): {e}", entry.connector))
        })?);
    }

    Ok((entities, policies))
}

fn csv_err(path: &Path, e: impl std::fmt::Display) -> ArborError {
    ArborError::ConversionError(format!("{}: {e}", path.display()))
}

fn parse_uuid(path: &Path, field: &str, value: &str) -> ArborResult<Uuid> {
    Uuid::parse_str(value.trim())
        .map_err(|e| csv_err(path, format!("invalid uuid in column {field:?} ({value:?}): {e}")))
}

/// Parses a `;`-separated list of UUIDs. Blank entries (including an entirely
/// empty field) are skipped.
fn parse_uuid_list(path: &Path, field: &str, value: &str) -> ArborResult<Vec<Uuid>> {
    value
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| parse_uuid(path, field, s))
        .collect()
}

/// Resolves a configured column name to its position in the CSV header row.
fn column_index(path: &Path, headers: &csv::StringRecord, field: &str, column_name: &str) -> ArborResult<usize> {
    headers.iter().position(|h| h == column_name).ok_or_else(|| {
        csv_err(
            path,
            format!("column mapping for {field:?} names {column_name:?}, which isn't a header in this file"),
        )
    })
}

fn get_field<'r>(path: &Path, record: &'r csv::StringRecord, idx: usize, field: &str) -> ArborResult<&'r str> {
    record
        .get(idx)
        .ok_or_else(|| csv_err(path, format!("row is missing column for {field:?} (index {idx})")))
}

/// Reads one entity-type CSV file per `columns`' mapping of logical fields
/// (`id`, `name`, `parent_id`) to that file's actual header names.
pub fn read_entities_csv(
    file: impl AsRef<Path>,
    entity_type: &str,
    columns: &CsvEntityColumns,
) -> ArborResult<Vec<EntityInput>> {
    let path = file.as_ref();
    let mut reader =
        csv::Reader::from_path(path).map_err(|e| csv_err(path, format!("failed to open: {e}")))?;

    let headers = reader
        .headers()
        .map_err(|e| csv_err(path, e))?
        .clone();
    let id_idx = column_index(path, &headers, "id", &columns.id)?;
    let name_idx = column_index(path, &headers, "name", &columns.name)?;
    let parent_idx = columns
        .parent_id
        .as_ref()
        .map(|c| column_index(path, &headers, "parent_id", c))
        .transpose()?;

    let mut out = Vec::new();
    for record in reader.records() {
        let record = record.map_err(|e| csv_err(path, e))?;
        let id = parse_uuid(path, "id", get_field(path, &record, id_idx, "id")?)?;
        let name = get_field(path, &record, name_idx, "name")?.to_string();
        let parents = match parent_idx {
            Some(idx) => {
                let raw = get_field(path, &record, idx, "parent_id")?.trim();
                if raw.is_empty() { vec![] } else { vec![parse_uuid(path, "parent_id", raw)?] }
            }
            None => vec![],
        };
        out.push(EntityInput { id, name, type_name: entity_type.to_string(), parents });
    }
    Ok(out)
}

fn parse_policy_target(path: &Path, column: &str, kind: &str, id: &str) -> ArborResult<PolicyTargetInput> {
    match kind.trim() {
        "entity" => Ok(PolicyTargetInput::Entity(parse_uuid(path, column, id)?)),
        "entity_with_descendants" => {
            Ok(PolicyTargetInput::EntityWithDescendants(parse_uuid(path, column, id)?))
        }
        "entity_type" => Ok(PolicyTargetInput::EntityType(id.trim().to_string())),
        "all" => Ok(PolicyTargetInput::All),
        other => Err(csv_err(
            path,
            format!("invalid target kind in column {column:?}: {other:?}"),
        )),
    }
}

/// Reads a policies CSV per `columns`' mapping of logical fields to that
/// file's actual header names.
pub fn read_policies_csv(file: impl AsRef<Path>, columns: &CsvPolicyColumns) -> ArborResult<Vec<PolicyInput>> {
    let path = file.as_ref();
    let mut reader =
        csv::Reader::from_path(path).map_err(|e| csv_err(path, format!("failed to open: {e}")))?;

    let headers = reader.headers().map_err(|e| csv_err(path, e))?.clone();
    let idx = |field: &str, column_name: &str| column_index(path, &headers, field, column_name);
    let id_idx = idx("id", &columns.id)?;
    let name_idx = idx("name", &columns.name)?;
    let policy_type_idx = idx("policy_type", &columns.policy_type)?;
    let principal_type_idx = idx("principal_type", &columns.principal_type)?;
    let principal_id_idx = idx("principal_id", &columns.principal_id)?;
    let resource_type_idx = idx("resource_type", &columns.resource_type)?;
    let resource_id_idx = idx("resource_id", &columns.resource_id)?;
    let actions_idx = idx("actions", &columns.actions)?;

    let mut out = Vec::new();
    for record in reader.records() {
        let record = record.map_err(|e| csv_err(path, e))?;
        let id = parse_uuid(path, "id", get_field(path, &record, id_idx, "id")?)?;
        let name = get_field(path, &record, name_idx, "name")?.to_string();
        let policy_type = match get_field(path, &record, policy_type_idx, "policy_type")?.trim() {
            "permit" => PolicyType::Permit,
            "forbid" => PolicyType::Forbid,
            other => {
                return Err(csv_err(
                    path,
                    format!("invalid policy_type {other:?} (expected \"permit\" or \"forbid\")"),
                ));
            }
        };
        let principal = parse_policy_target(
            path,
            "principal_id",
            get_field(path, &record, principal_type_idx, "principal_type")?,
            get_field(path, &record, principal_id_idx, "principal_id")?,
        )?;
        let resource = parse_policy_target(
            path,
            "resource_id",
            get_field(path, &record, resource_type_idx, "resource_type")?,
            get_field(path, &record, resource_id_idx, "resource_id")?,
        )?;
        let actions = parse_uuid_list(path, "actions", get_field(path, &record, actions_idx, "actions")?)?;

        out.push(PolicyInput { id, name, policy_type, principal, resource, actions });
    }
    Ok(out)
}
