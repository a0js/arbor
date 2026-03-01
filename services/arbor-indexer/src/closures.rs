use rapidhash::RapidHashMap;
use roaring::RoaringBitmap;
use std::collections::HashSet;

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

/// Compute all transitive descendants of `entity_idx` by following `children`.
///
/// The entity itself is NOT included.
pub fn compute_descendants(
    children: &RapidHashMap<u32, HashSet<u32>>,
    entity_idx: u32,
) -> RoaringBitmap {
    let mut descendants = RoaringBitmap::new();
    let mut stack = vec![entity_idx];

    while let Some(idx) = stack.pop() {
        if let Some(child_indices) = children.get(&idx) {
            for &child_idx in child_indices {
                if descendants.insert(child_idx) {
                    stack.push(child_idx);
                }
            }
        }
    }

    descendants
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
    fn test_compute_descendants_success() {
        // root(0) -> parent(1) -> child(2)
        let c = children(&[(0, 1), (1, 2)]);
        let descendants = compute_descendants(&c, 0);
        assert_eq!(descendants.len(), 2);
        assert!(descendants.contains(1));
        assert!(descendants.contains(2));
    }

    #[test]
    fn test_compute_descendants_leaf_has_none() {
        let c = children(&[(0, 1)]);
        assert!(compute_descendants(&c, 1).is_empty());
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

    #[test]
    fn test_compute_descendants_diamond_pattern() {
        // C(2) has children B(1) and A(0); B(1) has child A(0)
        let c = children(&[(2, 1), (2, 0), (1, 0)]);
        let descendants = compute_descendants(&c, 2);
        assert_eq!(descendants.len(), 2);
        assert!(descendants.contains(1));
        assert!(descendants.contains(0));
    }
}
