//! Loads a graph from `config/connectors.yaml` via `arbor-connectors`.
//!
//! Actions and action sets are ingested the same way as entities and
//! policies -- CSVs named in `data_model.yaml`'s `actions:`/`action_sets:`
//! sections, with IDs `arbor-connectors` derives once from each row's name --
//! so registration never has to independently derive an ID and hope it
//! matches whatever a dataset generator computed. If `data_model.yaml`
//! declares no `actions:` sources at all, this falls back to a standard
//! `read`/`write`/`delete` set scoped to `File`, for compatibility with data
//! models that predate the `actions:` section (e.g. `gen_company_dataset.rs`).
use std::path::Path;

use arbor_connectors::{ConnectorConfigFile, DataModelConfigFile};
use arbor_graph_core::graph::Graph;
use arbor_types::{Action, ActionInput, ActionSet, ArborResult};

pub const STANDARD_ACTIONS: &[&str] = &["read", "write", "delete"];
pub const STANDARD_ACTIONS_ENTITY_TYPE: &str = "File";

fn register_action(graph: &mut Graph, action: ActionInput) -> ArborResult<()> {
    let type_id = graph.get_or_create_entity_type_id(&action.type_name);
    graph.add_action(Action {
        id: action.id,
        name: action.name,
        entity_type_id: type_id,
        description: action.description,
    })
}

/// Builds a `Graph` from every entity type / action / action set / policy
/// source in `data_model`, resolved against the connectors in `connectors`.
/// Registers actions and action sets before policies, so policy rows can
/// reference them. Each connector's `file` path is resolved relative to
/// `base_dir`.
pub fn build_graph(
    connectors: &ConnectorConfigFile,
    data_model: &DataModelConfigFile,
    base_dir: impl AsRef<Path>,
) -> ArborResult<Graph> {
    let loaded = arbor_connectors::load_all(connectors, data_model, base_dir)?;

    let mut graph = Graph::new();

    for entity in loaded.entities {
        graph.upsert_entity_from_input(entity)?;
    }

    if loaded.actions.is_empty() {
        for name in STANDARD_ACTIONS {
            register_action(&mut graph, ActionInput {
                id: Action::hash_action_reference(&format!("action:{name}")),
                name: name.to_string(),
                type_name: STANDARD_ACTIONS_ENTITY_TYPE.to_string(),
                description: None,
            })?;
        }
    } else {
        for action in loaded.actions {
            register_action(&mut graph, action)?;
        }
    }

    for action_set in loaded.action_sets {
        graph.upsert_action_set(ActionSet {
            id: action_set.id,
            name: action_set.name,
            description: action_set.description,
            actions: action_set.actions,
        })?;
    }

    for policy in loaded.policies {
        graph.upsert_policy_from_input(policy)?;
    }

    Ok(graph)
}
