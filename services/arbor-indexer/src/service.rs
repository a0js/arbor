use std::fs;
use std::path::PathBuf;
use std::time::Instant;

use arbor_graph_core::graph::Graph;
use arbor_index_snapshot::RkyvPackagedSnapshot;
use arbor_types::ArborResult;

use crate::snapshot_builder::SnapshotBuilder;

/// The indexer service.
///
/// Owns the in-memory [`Graph`] and is responsible for rebuilding and writing
/// the snapshot file whenever the graph changes.
///
/// In the future, arbor-connectors will call [`IndexerService::apply`] (or
/// similar) to push entity/policy mutations into the graph and trigger a
/// snapshot rebuild.
pub struct IndexerService {
    graph: Graph,
    snapshot_path: PathBuf,
    generation: u64,
}

impl IndexerService {
    pub fn new(graph: Graph, snapshot_path: PathBuf) -> Self {
        Self { graph, snapshot_path, generation: 0 }
    }

    /// Rebuild the snapshot from the current graph state and write it to disk.
    ///
    /// Called once on startup and again after each graph mutation batch.
    pub fn rebuild_snapshot(&mut self) -> ArborResult<()> {
        self.generation += 1;

        let start = Instant::now();
        let snapshot = SnapshotBuilder::build(&self.graph)?;
        let generation_ms = start.elapsed().as_millis() as u64;

        let entity_count = snapshot.nodes.len();

        let packaged = RkyvPackagedSnapshot::from_snapshot(snapshot, self.generation, generation_ms)
            .map_err(|e| arbor_types::ArborError::ConversionError(e.to_string()))?;

        let bytes = packaged.serialize()
            .map_err(|e| arbor_types::ArborError::ConversionError(e.to_string()))?;

        fs::write(&self.snapshot_path, &bytes)
            .map_err(|e| arbor_types::ArborError::ConversionError(e.to_string()))?;

        println!(
            "Snapshot v{} written ({} nodes, {}ms build, {} bytes) → {}",
            self.generation,
            entity_count,
            generation_ms,
            bytes.len(),
            self.snapshot_path.display(),
        );

        Ok(())
    }

    /// Provides mutable access to the graph for seeding or future connector use.
    pub fn graph_mut(&mut self) -> &mut Graph {
        &mut self.graph
    }
}
