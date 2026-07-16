use std::env;
use std::path::PathBuf;

use arbor_indexer::csv_source;
use arbor_indexer::service::IndexerService;

mod example_graph;

#[tokio::main]
async fn main() {
    let snapshot_path = PathBuf::from(
        env::var("ARBOR_SNAPSHOT_PATH").unwrap_or_else(|_| "snapshot.arbor".to_string()),
    );

    // ARBOR_CONFIG_DIR points at a directory containing connectors.yaml +
    // data_model.yaml (loaded via arbor-connectors); otherwise fall back
    // to the hardcoded example graph.
    let graph = match env::var("ARBOR_CONFIG_DIR") {
        Ok(dir) => {
            println!("Loading graph from connectors in {dir}");
            let connectors = arbor_connectors::load_connector_config(&dir).expect("failed to load connectors.yaml");
            let data_model = arbor_connectors::load_data_model_config(&dir).expect("failed to load data_model.yaml");
            csv_source::build_graph(&connectors, &data_model, &dir).expect("failed to load graph from connectors")
        }
        Err(_) => example_graph::build(),
    };
    let mut svc = IndexerService::new(graph, snapshot_path);

    // Generate the first snapshot immediately on startup.
    svc.rebuild_snapshot().expect("initial snapshot build failed");

    println!("Indexer running. Waiting for shutdown signal (CTRL-C)...");

    // TODO: Replace this with a connector event loop that calls
    //       svc.graph_mut() + svc.rebuild_snapshot() on each mutation batch.
    tokio::signal::ctrl_c().await.expect("failed to listen for ctrl-c");

    println!("Shutting down.");
}
