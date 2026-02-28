use std::collections::{HashMap, HashSet};
use arbor_types::ArborResult;
use arbor_graph_core::graph::Graph;
use roaring::RoaringBitmap;
use uuid::Uuid;

pub fn compute_ancestors(
    graph: &Graph,
    entity_uuid: Uuid,
    parents_map: &HashMap<Uuid, Vec<Uuid>>,
) -> ArborResult<RoaringBitmap> {
    let mut ancestors = RoaringBitmap::new();
    let mut visited = HashSet::new();
    let mut path = HashSet::new();

    dfs_reachability(
        graph,
        &entity_uuid,
        parents_map,
        &mut visited,
        &mut path,
        &mut ancestors,
    )?;

    Ok(ancestors)
}

pub fn compute_descendants(
    graph: &Graph,
    entity_uuid: Uuid,
    children_map: &HashMap<Uuid, Vec<Uuid>>,
) -> ArborResult<RoaringBitmap> {
    let mut descendants = RoaringBitmap::new();
    let mut visited = HashSet::new();
    let mut path = HashSet::new();

    dfs_reachability(
        graph,
        &entity_uuid,
        children_map,
        &mut visited,
        &mut path,
        &mut descendants,
    )?;

    Ok(descendants)
}

fn dfs_reachability(
    graph: &Graph,
    current_uuid: &Uuid,
    adjacency_map: &HashMap<Uuid, Vec<Uuid>>,
    visited: &mut HashSet<Uuid>,
    path: &mut HashSet<Uuid>,
    results: &mut RoaringBitmap,
) -> ArborResult<()> {
    if path.contains(current_uuid) {
        return Err(arbor_types::ArborError::CircularDependency(format!(
            "Circular dependency detected at entity {}",
            current_uuid
        )));
    }

    if visited.contains(current_uuid) {
        return Ok(());
    }

    visited.insert(*current_uuid);
    path.insert(*current_uuid);

    if let Some(neighbors) = adjacency_map.get(current_uuid) {
        for neighbor_uuid in neighbors {
            if let Some(&index) = graph.uuid_to_index.get(neighbor_uuid) {
                results.insert(index);
            }
            dfs_reachability(graph, neighbor_uuid, adjacency_map, visited, path, results)?;
        }
    }

    path.remove(current_uuid);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use arbor_types::ArborError;

    #[test]
    fn test_compute_ancestors_success() {
        let mut graph = Graph::new();
        let root_id = Uuid::new_v4();
        let parent_id = Uuid::new_v4();
        let child_id = Uuid::new_v4();

        graph.uuid_to_index.insert(root_id, 0);
        graph.uuid_to_index.insert(parent_id, 1);
        graph.uuid_to_index.insert(child_id, 2);

        let mut parents_map = HashMap::new();
        parents_map.insert(child_id, vec![parent_id]);
        parents_map.insert(parent_id, vec![root_id]);

        let ancestors = compute_ancestors(&graph, child_id, &parents_map).unwrap();
        assert_eq!(ancestors.len(), 2);
        assert!(ancestors.contains(0)); // root
        assert!(ancestors.contains(1)); // parent
    }

    #[test]
    fn test_compute_ancestors_circular_self() {
        let mut graph = Graph::new();
        let a_id = Uuid::new_v4();

        graph.uuid_to_index.insert(a_id, 0);

        let mut parents_map = HashMap::new();
        parents_map.insert(a_id, vec![a_id]);

        let result = compute_ancestors(&graph, a_id, &parents_map);
        match result {
            Err(ArborError::CircularDependency(msg)) => {
                assert!(msg.contains("Circular dependency detected"));
            }
            _ => panic!("Expected CircularDependency error, got {:?}", result),
        }
    }

    #[test]
    fn test_compute_ancestors_circular_indirect() {
        let mut graph = Graph::new();
        let a_id = Uuid::new_v4();
        let b_id = Uuid::new_v4();

        graph.uuid_to_index.insert(a_id, 0);
        graph.uuid_to_index.insert(b_id, 1);

        let mut parents_map = HashMap::new();
        parents_map.insert(a_id, vec![b_id]);
        parents_map.insert(b_id, vec![a_id]);

        let result = compute_ancestors(&graph, a_id, &parents_map);
        match result {
            Err(ArborError::CircularDependency(msg)) => {
                assert!(msg.contains("Circular dependency detected"));
            }
            _ => panic!("Expected CircularDependency error, got {:?}", result),
        }
    }

    #[test]
    fn test_compute_descendants_success() {
        let mut graph = Graph::new();
        let root_id = Uuid::new_v4();
        let parent_id = Uuid::new_v4();
        let child_id = Uuid::new_v4();

        graph.uuid_to_index.insert(root_id, 0);
        graph.uuid_to_index.insert(parent_id, 1);
        graph.uuid_to_index.insert(child_id, 2);

        let mut children_map = HashMap::new();
        children_map.insert(root_id, vec![parent_id]);
        children_map.insert(parent_id, vec![child_id]);

        let descendants = compute_descendants(&graph, root_id, &children_map).unwrap();
        assert_eq!(descendants.len(), 2);
        assert!(descendants.contains(1)); // parent
        assert!(descendants.contains(2)); // child
    }

    #[test]
    fn test_compute_descendants_circular_indirect() {
        let mut graph = Graph::new();
        let a_id = Uuid::new_v4();
        let b_id = Uuid::new_v4();

        graph.uuid_to_index.insert(a_id, 0);
        graph.uuid_to_index.insert(b_id, 1);

        let mut children_map = HashMap::new();
        children_map.insert(a_id, vec![b_id]);
        children_map.insert(b_id, vec![a_id]);

        let result = compute_descendants(&graph, a_id, &children_map);
        match result {
            Err(ArborError::CircularDependency(msg)) => {
                assert!(msg.contains("Circular dependency detected"));
            }
            _ => panic!("Expected CircularDependency error, got {:?}", result),
        }
    }

    #[test]
    fn test_compute_ancestors_diamond_pattern() {
        let mut graph = Graph::new();
        let a_id = Uuid::new_v4();
        let b_id = Uuid::new_v4();
        let c_id = Uuid::new_v4();

        graph.uuid_to_index.insert(a_id, 0);
        graph.uuid_to_index.insert(b_id, 1);
        graph.uuid_to_index.insert(c_id, 2);

        let mut parents_map = HashMap::new();
        parents_map.insert(a_id, vec![b_id, c_id]);
        parents_map.insert(b_id, vec![c_id]);

        let ancestors = compute_ancestors(&graph, a_id, &parents_map).unwrap();
        assert_eq!(ancestors.len(), 2);
        assert!(ancestors.contains(1)); // B
        assert!(ancestors.contains(2)); // C
    }

    #[test]
    fn test_compute_descendants_diamond_pattern() {
        let mut graph = Graph::new();
        let a_id = Uuid::new_v4();
        let b_id = Uuid::new_v4();
        let c_id = Uuid::new_v4();

        graph.uuid_to_index.insert(a_id, 0);
        graph.uuid_to_index.insert(b_id, 1);
        graph.uuid_to_index.insert(c_id, 2);

        let mut children_map = HashMap::new();
        children_map.insert(c_id, vec![b_id, a_id]);
        children_map.insert(b_id, vec![a_id]);

        let descendants = compute_descendants(&graph, c_id, &children_map).unwrap();
        assert_eq!(descendants.len(), 2);
        assert!(descendants.contains(1)); // B
        assert!(descendants.contains(0)); // A
    }
}