//! Loads a graph from `config/connectors.yaml` via `arbor-connectors`.
//!
//! Actions aren't part of the connector config -- they're a small, fixed
//! vocabulary for the deployment rather than ingested data -- so this
//! registers a standard `read`/`write`/`delete` action set scoped to the
//! `File` entity type before loading policies. Policy sources reference
//! these by the same deterministic IDs (`Action::hash_action_reference`), so
//! a dataset generator can compute matching action UUIDs without a shared
//! actions file.
use std::path::Path;

use arbor_connectors::{ConnectorConfigFile, DataModelConfigFile};
use arbor_graph_core::graph::Graph;
use arbor_types::{Action, ArborResult};

pub const STANDARD_ACTIONS: &[&str] = &["read", "write", "delete"];

fn register_standard_actions(graph: &mut Graph, scoped_type: &str) -> ArborResult<()> {
    let type_id = graph.get_or_create_entity_type_id(scoped_type);
    for name in STANDARD_ACTIONS {
        let id = Action::hash_action_reference(&format!("action:{name}"));
        graph.add_action(Action {
            id,
            name: name.to_string(),
            entity_type_id: type_id,
            description: None,
        })?;
    }
    Ok(())
}

/// Builds a `Graph` from every entity type / policy source in `data_model`,
/// resolved against the connectors in `connectors`. Registers the standard
/// action set scoped to `File` first so policy rows can reference it. Each
/// connector's `file` path is resolved relative to `base_dir`.
pub fn build_graph(
    connectors: &ConnectorConfigFile,
    data_model: &DataModelConfigFile,
    base_dir: impl AsRef<Path>,
) -> ArborResult<Graph> {
    let (entity_inputs, policy_inputs) = arbor_connectors::load_all(connectors, data_model, base_dir)?;

    let mut graph = Graph::new();

    for entity in entity_inputs {
        graph.upsert_entity_from_input(entity)?;
    }

    register_standard_actions(&mut graph, "File")?;

    for policy in policy_inputs {
        graph.upsert_policy_from_input(policy)?;
    }

    Ok(graph)
}
