use rapidhash::RapidHashMap;
use roaring::RoaringBitmap;
use std::collections::{HashSet, VecDeque};

/// Compute all transitive ancestors of `entity_idx` by following `parents`.
///
/// The entity itself is NOT included — callers add it when self-inclusion is
/// required (the snapshot builder inserts the entity's own index into the
/// returned bitmap before storing it).
///
/// Circular dependencies are impossible here: the graph validates acyclicity
/// on every `upsert_entity` call, so the `visited` check via bitmap insertion
/// is only needed to handle shared ancestors in DAG (diamond) patterns.
pub fn compute_ancestors(
    parents: &RapidHashMap<u32, HashSet<u32>>,
    entity_idx: u32,
) -> RoaringBitmap {
    let mut ancestors = RoaringBitmap::new();
    let mut stack = vec![entity_idx];

    while let Some(idx) = stack.pop() {
        if let Some(parent_indices) = parents.get(&idx) {
            for &parent_idx in parent_indices {
                if ancestors.insert(parent_idx) {
                    stack.push(parent_idx);
                }
            }
        }
    }

    ancestors
}

/// Compute transitive descendants for every node in one topological pass.
///
/// Processes nodes leaves-first so each edge is visited exactly once, giving
/// O(N + E) total work instead of O(N × avg_subtree_size).
///
/// Returns a `Vec<RoaringBitmap>` of length `node_count`; index `i` holds
/// all transitive descendants of node `i`.  The node itself is NOT included.
pub fn compute_all_descendants(
    children: &RapidHashMap<u32, HashSet<u32>>,
    node_count: usize,
) -> Vec<RoaringBitmap> {
    // remaining_children[i]: how many children of i are not yet processed.
    let mut remaining_children = vec![0u32; node_count];
    // notify_parents[i]: parents to update when node i is done.
    let mut notify_parents: Vec<Vec<u32>> = vec![Vec::new(); node_count];

    for (&parent, child_set) in children {
        let p = parent as usize;
        if p >= node_count {
            continue;
        }
        remaining_children[p] = child_set.len() as u32;
        for &child in child_set {
            let c = child as usize;
            if c < node_count {
                notify_parents[c].push(parent);
            }
        }
    }

    let mut result = vec![RoaringBitmap::new(); node_count];

    // Seed with leaves: nodes that have no children.
    let mut queue: VecDeque<u32> = (0..node_count as u32)
        .filter(|&i| remaining_children[i as usize] == 0)
        .collect();

    while let Some(idx) = queue.pop_front() {
        let desc = result[idx as usize].clone();
        for &parent in &notify_parents[idx as usize] {
            let p = parent as usize;
            result[p] |= &desc;
            result[p].insert(idx);
            remaining_children[p] -= 1;
            if remaining_children[p] == 0 {
                queue.push_back(parent);
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use rapidhash::RapidHashMap;

    fn parents(edges: &[(u32, u32)]) -> RapidHashMap<u32, HashSet<u32>> {
        let mut map: RapidHashMap<u32, HashSet<u32>> = RapidHashMap::default();
        for &(child, parent) in edges {
            map.entry(child).or_default().insert(parent);
        }
        map
    }

    fn children(edges: &[(u32, u32)]) -> RapidHashMap<u32, HashSet<u32>> {
        let mut map: RapidHashMap<u32, HashSet<u32>> = RapidHashMap::default();
        for &(parent, child) in edges {
            map.entry(parent).or_default().insert(child);
        }
        map
    }

    #[test]
    fn test_compute_ancestors_success() {
        // root(0) <- parent(1) <- child(2)
        let p = parents(&[(2, 1), (1, 0)]);
        let ancestors = compute_ancestors(&p, 2);
        assert_eq!(ancestors.len(), 2);
        assert!(ancestors.contains(0));
        assert!(ancestors.contains(1));
    }

    #[test]
    fn test_compute_ancestors_root_has_none() {
        let p = parents(&[(1, 0)]);
        assert!(compute_ancestors(&p, 0).is_empty());
    }

    #[test]
    fn test_compute_ancestors_diamond_pattern() {
        // A(0) has parents B(1) and C(2); B(1) has parent C(2)
        let p = parents(&[(0, 1), (0, 2), (1, 2)]);
        let ancestors = compute_ancestors(&p, 0);
        assert_eq!(ancestors.len(), 2);
        assert!(ancestors.contains(1));
        assert!(ancestors.contains(2));
    }

}
