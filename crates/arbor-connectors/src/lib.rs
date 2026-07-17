mod condition_parser;
pub use condition_parser::parse_condition;

use std::collections::HashMap;
use std::path::Path;

use config::{Config, Environment};
use serde::Deserialize;
use uuid::Uuid;

use arbor_types::{
    Action, ActionInput, ActionSetInput, ArborError, ArborResult, AttributeInput, AttributeValueInput,
    EntityInput, PolicyInput, PolicyTargetInput, PolicyType,
};

/// One CSV holds rows of exactly one entity type (e.g. `employees.csv` is all
/// `Employee`s). Most exports have a single "manager_id" / "parent_id" style
/// column, so that's supported directly; sources with genuine multi-parent
/// structure (e.g. group membership layered on an org chart) can instead use
/// `parent_ids`, a `;`-separated UUID list mirroring `CsvPolicyColumns::actions`.
/// Both may be set on the same mapping -- the resulting parents are their
/// union, deduplicated. Column *names* are declared here rather than assumed,
/// since a real export's headers won't match Arbor's internal field names.
#[derive(Debug, Deserialize)]
pub struct CsvEntityColumns {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub parent_ids: Option<String>,
    /// Attribute columns for this entity type, if any. Each entry declares one
    /// scalar column and the (possibly nested) attribute path it fills --
    /// nesting comes entirely from `path` having multiple dot-separated
    /// segments, never from the column's own value (no embedded JSON/objects).
    #[serde(default)]
    pub attributes: Vec<CsvAttributeColumn>,
}

/// One declared attribute: a dotted `path` (e.g. `"consent_flags.share_with_specialists"`),
/// which CSV `column` holds its value, and the value's `value_type` (there's no way to
/// infer a scalar's type from a raw CSV string alone, so this is explicit, not guessed).
#[derive(Debug, Deserialize)]
pub struct CsvAttributeColumn {
    pub path: String,
    pub column: String,
    pub value_type: AttributeColumnType,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttributeColumnType {
    String,
    Integer,
    Float,
    Bool,
}

/// Actions are referenced by name, the way a real export would (`"read_chart"`,
/// not an internal Arbor UUID) -- `entity_type` is descriptive metadata only
/// (mirroring `Action.entity_type_id`), not part of how the action is
/// identified. The UUID is derived once, by `action_id_for_name` below, and
/// used consistently by both this file's ingestion and by
/// `CsvPolicyColumns::actions`'s name lookups -- one function, one place, so
/// there's nothing for two independent call sites to disagree on.
#[derive(Debug, Deserialize)]
pub struct CsvActionColumns {
    pub name: String,
    pub entity_type: String,
    #[serde(default)]
    pub description: Option<String>,
}

/// Derives an action's UUID from its name alone (entity_type is descriptive,
/// not part of identity) -- the single place this is computed, so ingesting
/// `actions.csv` and resolving `CsvPolicyColumns::actions`'s name list always
/// agree without either side having to know about the other. Public so
/// external tooling (e.g. a dataset's verification suite) can compute a
/// matching action UUID without re-deriving this formula independently --
/// the exact class of duplication that caused this connector's action IDs to
/// silently diverge once already.
pub fn action_id_for_name(name: &str) -> Uuid {
    Action::hash_action_reference(&format!("action:{name}"))
}

/// An action set's own UUID, derived from its name the same way an action's
/// is -- but hashed with a distinct prefix so a set can never collide with
/// an action that happens to share its name. Public for the same reason as
/// `action_id_for_name`.
pub fn action_set_id_for_name(name: &str) -> Uuid {
    Action::hash_action_reference(&format!("action_set:{name}"))
}

/// A named, reusable bundle of actions (e.g. "ConsultAccess" =
/// `read_labs;read_imaging`) that a policy can reference by name instead of
/// repeating its member actions on every row that grants the same bundle.
#[derive(Debug, Deserialize)]
pub struct CsvActionSetColumns {
    pub name: String,
    /// `;`-separated action *names* (resolved via `action_id_for_name`,
    /// same as `CsvPolicyColumns::actions`).
    pub actions: String,
    #[serde(default)]
    pub description: Option<String>,
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
    /// A single column holding a `;`-separated list of action *names*
    /// (resolved to UUIDs via `action_id_for_name`) -- unlike parents, a
    /// policy's action set genuinely is multi-valued.
    pub actions: String,
    /// Optional `;`-separated list of action-set *names* (resolved via
    /// `action_set_id_for_name`), for policies that grant a named bundle
    /// rather than (or in addition to) individual actions.
    #[serde(default)]
    pub action_sets: Option<String>,
    /// Optional free-text ABAC condition, parsed via `condition_parser::parse_condition`
    /// (see that module for the grammar). Attribute paths and `in_hierarchy`'s
    /// UUID are resolved against the graph at build time, same deferred-resolution
    /// pattern as everything else this connector ingests.
    #[serde(default)]
    pub condition: Option<String>,
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

/// One action source: which connector to read it from, and its column
/// mapping. Ingested the same way as entities/policies -- a real UUID column
/// per row, not a name-derived hash -- so a dataset's actions are declared
/// once, in one file, rather than recomputed by both a generator and the
/// indexer from a hashing convention the two have to agree on.
#[derive(Debug, Deserialize)]
pub struct ActionSourceEntry {
    pub connector: String,
    pub columns: CsvActionColumns,
}

/// One action-set source: which connector to read it from, and its column mapping.
#[derive(Debug, Deserialize)]
pub struct ActionSetSourceEntry {
    pub connector: String,
    pub columns: CsvActionSetColumns,
}

/// Both entity types and policies are "data model" config, as opposed to
/// `connectors.yaml`'s connection info -- kept in one file so the name
/// doesn't imply it's entity-types-only. `actions` defaults to empty for
/// backward compatibility with data models that rely on a caller-supplied
/// fallback action set (see `services/arbor-indexer/src/csv_source.rs`).
#[derive(Debug, Deserialize, Default)]
pub struct DataModelConfigFile {
    #[serde(default)]
    pub entity_types: Vec<EntityTypeEntry>,
    #[serde(default)]
    pub policies: Vec<PolicySourceEntry>,
    #[serde(default)]
    pub actions: Vec<ActionSourceEntry>,
    #[serde(default)]
    pub action_sets: Vec<ActionSetSourceEntry>,
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

/// Everything `load_all` reads out of a data model, grouped by kind. A named
/// struct rather than a tuple -- four same-shaped `Vec`s in a row invites
/// mixing up positions at the call site.
pub struct LoadedData {
    pub entities: Vec<EntityInput>,
    pub actions: Vec<ActionInput>,
    pub action_sets: Vec<ActionSetInput>,
    pub policies: Vec<PolicyInput>,
}

/// Reads every entity, action, action set, and policy named in `data_model`,
/// resolving each entry's `connector` against `connectors` and joining its
/// `file` against `base_dir`. Validates connector references up front
/// (before opening any files) so a typo'd connector name fails fast with the
/// entry that named it.
pub fn load_all(
    connectors: &ConnectorConfigFile,
    data_model: &DataModelConfigFile,
    base_dir: impl AsRef<Path>,
) -> ArborResult<LoadedData> {
    let base = base_dir.as_ref();

    for entry in &data_model.entity_types {
        resolve_csv_file(connectors, &entry.connector).map_err(|e| {
            ArborError::ConversionError(format!("entity_type {:?}: {e}", entry.name))
        })?;
    }
    for entry in &data_model.actions {
        resolve_csv_file(connectors, &entry.connector).map_err(|e| {
            ArborError::ConversionError(format!("action source (connector {:?}): {e}", entry.connector))
        })?;
    }
    for entry in &data_model.action_sets {
        resolve_csv_file(connectors, &entry.connector).map_err(|e| {
            ArborError::ConversionError(format!("action set source (connector {:?}): {e}", entry.connector))
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

    let mut actions = Vec::new();
    for entry in &data_model.actions {
        let file = resolve_csv_file(connectors, &entry.connector)?;
        actions.extend(read_actions_csv(base.join(file), &entry.columns).map_err(|e| {
            ArborError::ConversionError(format!("action source (connector {:?}): {e}", entry.connector))
        })?);
    }

    let mut action_sets = Vec::new();
    for entry in &data_model.action_sets {
        let file = resolve_csv_file(connectors, &entry.connector)?;
        action_sets.extend(read_action_sets_csv(base.join(file), &entry.columns).map_err(|e| {
            ArborError::ConversionError(format!("action set source (connector {:?}): {e}", entry.connector))
        })?);
    }

    let mut policies = Vec::new();
    for entry in &data_model.policies {
        let file = resolve_csv_file(connectors, &entry.connector)?;
        policies.extend(read_policies_csv(base.join(file), &entry.columns).map_err(|e| {
            ArborError::ConversionError(format!("policy source (connector {:?}): {e}", entry.connector))
        })?);
    }

    Ok(LoadedData { entities, actions, action_sets, policies })
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

/// Parses a `;`-separated list of action names into their UUIDs via
/// `action_id_for_name`. Blank entries (including an entirely empty field)
/// are skipped. Unlike `parse_uuid_list`, this can't fail -- any non-blank
/// name resolves to *some* UUID, whether or not `actions.csv` happens to
/// declare an action by that name (an unregistered action ID just never
/// matches any policy at eval time, the same way a typo'd UUID would).
fn parse_action_name_list(value: &str) -> Vec<Uuid> {
    value
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(action_id_for_name)
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
/// (`id`, `name`, `parent_id`, `parent_ids`) to that file's actual header names.
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
    let parent_ids_idx = columns
        .parent_ids
        .as_ref()
        .map(|c| column_index(path, &headers, "parent_ids", c))
        .transpose()?;
    let attribute_idxs: Vec<(Vec<String>, usize, &AttributeColumnType)> = columns
        .attributes
        .iter()
        .map(|attr| -> ArborResult<_> {
            let idx = column_index(path, &headers, &attr.path, &attr.column)?;
            let path_segments = attr.path.split('.').map(str::to_string).collect();
            Ok((path_segments, idx, &attr.value_type))
        })
        .collect::<ArborResult<Vec<_>>>()?;

    let mut out = Vec::new();
    for record in reader.records() {
        let record = record.map_err(|e| csv_err(path, e))?;
        let id = parse_uuid(path, "id", get_field(path, &record, id_idx, "id")?)?;
        let name = get_field(path, &record, name_idx, "name")?.to_string();

        let mut parents = Vec::new();
        if let Some(idx) = parent_idx {
            let raw = get_field(path, &record, idx, "parent_id")?.trim();
            if !raw.is_empty() {
                parents.push(parse_uuid(path, "parent_id", raw)?);
            }
        }
        if let Some(idx) = parent_ids_idx {
            let raw = get_field(path, &record, idx, "parent_ids")?;
            parents.extend(parse_uuid_list(path, "parent_ids", raw)?);
        }
        parents.sort();
        parents.dedup();

        let mut attributes = Vec::new();
        for (path_segments, idx, value_type) in &attribute_idxs {
            let raw = get_field(path, &record, *idx, "attributes")?.trim();
            let value = match value_type {
                AttributeColumnType::String => AttributeValueInput::String(raw.to_string()),
                AttributeColumnType::Integer => AttributeValueInput::Integer(
                    raw.parse().map_err(|e| csv_err(path, format!("invalid integer in attribute column ({raw:?}): {e}")))?,
                ),
                AttributeColumnType::Float => AttributeValueInput::Float(
                    raw.parse::<f64>()
                        .map(ordered_float::OrderedFloat)
                        .map_err(|e| csv_err(path, format!("invalid float in attribute column ({raw:?}): {e}")))?,
                ),
                AttributeColumnType::Bool => AttributeValueInput::Bool(
                    raw.parse().map_err(|e| csv_err(path, format!("invalid bool in attribute column ({raw:?}): {e}")))?,
                ),
            };
            attributes.push(AttributeInput { path: path_segments.clone(), value });
        }

        out.push(EntityInput { id, name, type_name: entity_type.to_string(), parents, attributes });
    }
    Ok(out)
}

/// Reads one actions CSV per `columns`' mapping of logical fields (`name`,
/// `entity_type`, `description`) to that file's actual header names. The
/// UUID is derived from `name` via `action_id_for_name`, not read from a
/// column -- see that function's doc for why.
pub fn read_actions_csv(file: impl AsRef<Path>, columns: &CsvActionColumns) -> ArborResult<Vec<ActionInput>> {
    let path = file.as_ref();
    let mut reader =
        csv::Reader::from_path(path).map_err(|e| csv_err(path, format!("failed to open: {e}")))?;

    let headers = reader.headers().map_err(|e| csv_err(path, e))?.clone();
    let name_idx = column_index(path, &headers, "name", &columns.name)?;
    let entity_type_idx = column_index(path, &headers, "entity_type", &columns.entity_type)?;
    let description_idx = columns
        .description
        .as_ref()
        .map(|c| column_index(path, &headers, "description", c))
        .transpose()?;

    let mut out = Vec::new();
    for record in reader.records() {
        let record = record.map_err(|e| csv_err(path, e))?;
        let name = get_field(path, &record, name_idx, "name")?.to_string();
        let type_name = get_field(path, &record, entity_type_idx, "entity_type")?.to_string();
        let description = match description_idx {
            Some(idx) => {
                let raw = get_field(path, &record, idx, "description")?.trim();
                if raw.is_empty() { None } else { Some(raw.to_string()) }
            }
            None => None,
        };
        out.push(ActionInput { id: action_id_for_name(&name), name, type_name, description });
    }
    Ok(out)
}

/// Reads one action-sets CSV per `columns`' mapping of logical fields
/// (`name`, `actions`, `description`) to that file's actual header names.
/// Member actions are resolved via `action_id_for_name`, same as
/// `CsvPolicyColumns::actions` -- so a set's members don't need to be
/// declared in `actions.csv` for this to succeed, any more than a policy's
/// loose actions do.
pub fn read_action_sets_csv(
    file: impl AsRef<Path>,
    columns: &CsvActionSetColumns,
) -> ArborResult<Vec<ActionSetInput>> {
    let path = file.as_ref();
    let mut reader =
        csv::Reader::from_path(path).map_err(|e| csv_err(path, format!("failed to open: {e}")))?;

    let headers = reader.headers().map_err(|e| csv_err(path, e))?.clone();
    let name_idx = column_index(path, &headers, "name", &columns.name)?;
    let actions_idx = column_index(path, &headers, "actions", &columns.actions)?;
    let description_idx = columns
        .description
        .as_ref()
        .map(|c| column_index(path, &headers, "description", c))
        .transpose()?;

    let mut out = Vec::new();
    for record in reader.records() {
        let record = record.map_err(|e| csv_err(path, e))?;
        let name = get_field(path, &record, name_idx, "name")?.to_string();
        let actions = parse_action_name_list(get_field(path, &record, actions_idx, "actions")?);
        let description = match description_idx {
            Some(idx) => {
                let raw = get_field(path, &record, idx, "description")?.trim();
                if raw.is_empty() { None } else { Some(raw.to_string()) }
            }
            None => None,
        };
        out.push(ActionSetInput { id: action_set_id_for_name(&name), name, description, actions });
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
    let action_sets_idx = columns
        .action_sets
        .as_ref()
        .map(|c| idx("action_sets", c))
        .transpose()?;
    let condition_idx = columns
        .condition
        .as_ref()
        .map(|c| idx("condition", c))
        .transpose()?;

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
        let actions = parse_action_name_list(get_field(path, &record, actions_idx, "actions")?);
        let action_sets = match action_sets_idx {
            Some(idx) => get_field(path, &record, idx, "action_sets")?
                .split(';')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(action_set_id_for_name)
                .collect(),
            None => Vec::new(),
        };

        let condition = match condition_idx {
            Some(idx) => {
                let raw = get_field(path, &record, idx, "condition")?.trim();
                if raw.is_empty() { None } else { Some(condition_parser::parse_condition(raw)?) }
            }
            None => None,
        };

        out.push(PolicyInput { id, name, policy_type, principal, resource, actions, action_sets, condition });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn write_csv(contents: &str) -> std::path::PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "arbor-connectors-test-{}-{n}.csv",
            std::process::id()
        ));
        std::fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn single_parent_id_column() {
        let alice = "00000000-0000-0000-0000-000000000001";
        let bob = "00000000-0000-0000-0000-000000000002";
        let path = write_csv(&format!(
            "emp_id,full_name,manager_id\n{alice},Alice,\n{bob},Bob,{alice}\n"
        ));
        let columns = CsvEntityColumns {
            id: "emp_id".into(),
            name: "full_name".into(),
            parent_id: Some("manager_id".into()),
            parent_ids: None,
            attributes: vec![],
        };
        let entities = read_entities_csv(&path, "Employee", &columns).unwrap();
        std::fs::remove_file(&path).ok();

        assert_eq!(entities.len(), 2);
        assert_eq!(entities[0].parents, Vec::<Uuid>::new());
        assert_eq!(
            entities[1].parents,
            vec![Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap()]
        );
    }

    #[test]
    fn parent_ids_column_supports_multiple_parents() {
        let g1 = "00000000-0000-0000-0000-0000000000a1";
        let g2 = "00000000-0000-0000-0000-0000000000a2";
        let path = write_csv(&format!(
            "id,name,group_ids\n00000000-0000-0000-0000-0000000000e1,Carol,{g1};{g2}\n"
        ));
        let columns = CsvEntityColumns {
            id: "id".into(),
            name: "name".into(),
            parent_id: None,
            parent_ids: Some("group_ids".into()),
            attributes: vec![],
        };
        let entities = read_entities_csv(&path, "Provider", &columns).unwrap();
        std::fs::remove_file(&path).ok();

        assert_eq!(entities.len(), 1);
        let mut parents = entities[0].parents.clone();
        parents.sort();
        let mut expected = vec![Uuid::parse_str(g1).unwrap(), Uuid::parse_str(g2).unwrap()];
        expected.sort();
        assert_eq!(parents, expected);
    }

    #[test]
    fn parent_id_and_parent_ids_combine_and_dedup() {
        let clinic = "00000000-0000-0000-0000-0000000000b1";
        let group = "00000000-0000-0000-0000-0000000000b2";
        // parent_id repeats one of the parent_ids entries -- should collapse to 2, not 3.
        let path = write_csv(&format!(
            "id,name,clinic_id,group_ids\n00000000-0000-0000-0000-0000000000e2,Dana,{clinic},{clinic};{group}\n"
        ));
        let columns = CsvEntityColumns {
            id: "id".into(),
            name: "name".into(),
            parent_id: Some("clinic_id".into()),
            parent_ids: Some("group_ids".into()),
            attributes: vec![],
        };
        let entities = read_entities_csv(&path, "Provider", &columns).unwrap();
        std::fs::remove_file(&path).ok();

        assert_eq!(entities.len(), 1);
        let mut parents = entities[0].parents.clone();
        parents.sort();
        let mut expected = vec![Uuid::parse_str(clinic).unwrap(), Uuid::parse_str(group).unwrap()];
        expected.sort();
        assert_eq!(parents, expected);
    }

    #[test]
    fn actions_csv_derives_id_from_name() {
        let path = write_csv(
            "action_name,scoped_type,notes\nread_chart,Patient,\nwrite_chart,Patient,clinical staff only\n",
        );
        let columns = CsvActionColumns {
            name: "action_name".into(),
            entity_type: "scoped_type".into(),
            description: Some("notes".into()),
        };
        let actions = read_actions_csv(&path, &columns).unwrap();
        std::fs::remove_file(&path).ok();

        assert_eq!(actions.len(), 2);
        assert_eq!(actions[0].id, action_id_for_name("read_chart"));
        assert_eq!(actions[0].name, "read_chart");
        assert_eq!(actions[0].type_name, "Patient");
        assert_eq!(actions[0].description, None);
        assert_eq!(actions[1].description, Some("clinical staff only".to_string()));
    }

    #[test]
    fn policy_actions_column_resolves_names_to_matching_action_ids() {
        let path = write_csv(
            "policy_id,policy_name,ptype,principal_kind,principal,resource_kind,resource,action_names\n\
             00000000-0000-0000-0000-0000000000f1,test-policy,permit,all,,entity_type,Patient,read_chart;read_labs\n",
        );
        let columns = CsvPolicyColumns {
            id: "policy_id".into(),
            name: "policy_name".into(),
            policy_type: "ptype".into(),
            principal_type: "principal_kind".into(),
            principal_id: "principal".into(),
            resource_type: "resource_kind".into(),
            resource_id: "resource".into(),
            actions: "action_names".into(),
            action_sets: None,
            condition: None,
        };
        let policies = read_policies_csv(&path, &columns).unwrap();
        std::fs::remove_file(&path).ok();

        assert_eq!(policies.len(), 1);
        assert_eq!(
            policies[0].actions,
            vec![action_id_for_name("read_chart"), action_id_for_name("read_labs")]
        );
        assert!(policies[0].action_sets.is_empty());
    }

    #[test]
    fn action_sets_csv_resolves_member_actions_by_name() {
        let path = write_csv("set_name,member_actions\nConsultAccess,read_labs;read_imaging\n");
        let columns = CsvActionSetColumns {
            name: "set_name".into(),
            actions: "member_actions".into(),
            description: None,
        };
        let sets = read_action_sets_csv(&path, &columns).unwrap();
        std::fs::remove_file(&path).ok();

        assert_eq!(sets.len(), 1);
        assert_eq!(sets[0].id, action_set_id_for_name("ConsultAccess"));
        assert_eq!(sets[0].name, "ConsultAccess");
        assert_eq!(
            sets[0].actions,
            vec![action_id_for_name("read_labs"), action_id_for_name("read_imaging")]
        );
        // A set's ID must never collide with an action of the same name.
        assert_ne!(sets[0].id, action_id_for_name("ConsultAccess"));
    }

    #[test]
    fn policy_action_sets_column_resolves_names_to_matching_set_ids() {
        let path = write_csv(
            "policy_id,policy_name,ptype,principal_kind,principal,resource_kind,resource,action_names,set_names\n\
             00000000-0000-0000-0000-0000000000f2,test-policy,permit,all,,entity_type,Patient,,ConsultAccess\n",
        );
        let columns = CsvPolicyColumns {
            id: "policy_id".into(),
            name: "policy_name".into(),
            policy_type: "ptype".into(),
            principal_type: "principal_kind".into(),
            principal_id: "principal".into(),
            resource_type: "resource_kind".into(),
            resource_id: "resource".into(),
            actions: "action_names".into(),
            action_sets: Some("set_names".into()),
            condition: None,
        };
        let policies = read_policies_csv(&path, &columns).unwrap();
        std::fs::remove_file(&path).ok();

        assert_eq!(policies.len(), 1);
        assert!(policies[0].actions.is_empty());
        assert_eq!(policies[0].action_sets, vec![action_set_id_for_name("ConsultAccess")]);
    }

    #[test]
    fn no_parent_columns_configured_yields_root_entity() {
        let path = write_csv("id,name\n00000000-0000-0000-0000-0000000000e3,Root\n");
        let columns = CsvEntityColumns {
            id: "id".into(),
            name: "name".into(),
            parent_id: None,
            parent_ids: None,
            attributes: vec![],
        };
        let entities = read_entities_csv(&path, "Clinic", &columns).unwrap();
        std::fs::remove_file(&path).ok();

        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0].parents, Vec::<Uuid>::new());
    }

    #[test]
    fn attribute_columns_produce_nested_and_scalar_attributes() {
        let path = write_csv(
            "id,name,share_flag,age\n00000000-0000-0000-0000-0000000000f9,Carol,true,42\n",
        );
        let columns = CsvEntityColumns {
            id: "id".into(),
            name: "name".into(),
            parent_id: None,
            parent_ids: None,
            attributes: vec![
                CsvAttributeColumn {
                    path: "consent_flags.share_with_specialists".into(),
                    column: "share_flag".into(),
                    value_type: AttributeColumnType::Bool,
                },
                CsvAttributeColumn {
                    path: "age".into(),
                    column: "age".into(),
                    value_type: AttributeColumnType::Integer,
                },
            ],
        };
        let entities = read_entities_csv(&path, "Patient", &columns).unwrap();
        std::fs::remove_file(&path).ok();

        assert_eq!(entities.len(), 1);
        let attrs = &entities[0].attributes;
        assert_eq!(attrs.len(), 2);

        let nested = attrs.iter().find(|a| a.path == vec!["consent_flags", "share_with_specialists"]).unwrap();
        assert!(matches!(nested.value, AttributeValueInput::Bool(true)));

        let age = attrs.iter().find(|a| a.path == vec!["age"]).unwrap();
        assert!(matches!(age.value, AttributeValueInput::Integer(42)));
    }
}
