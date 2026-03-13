use std::env;
use std::path::PathBuf;

use service::IndexerService;

mod example_graph;
mod service;

#[tokio::main]
async fn main() {
    let snapshot_path = PathBuf::from(
        env::var("ARBOR_SNAPSHOT_PATH").unwrap_or_else(|_| "snapshot.arbor".to_string()),
    );

    // Build initial graph. In the future this will be replaced by loading
    // state from a database via arbor-connectors.
    let graph = example_graph::build();
    let mut svc = IndexerService::new(graph, snapshot_path);

    // Generate the first snapshot immediately on startup.
    svc.rebuild_snapshot().expect("initial snapshot build failed");

    println!("Indexer running. Waiting for shutdown signal (CTRL-C)...");

    // TODO: Replace this with a connector event loop that calls
    //       svc.graph_mut() + svc.rebuild_snapshot() on each mutation batch.
    tokio::signal::ctrl_c().await.expect("failed to listen for ctrl-c");

    println!("Shutting down.");
}
