//! Builds the real scenario, optionally strips specific components to empty,
//! then serializes -- so `load_snapshot` on the stripped file measures
//! exactly how much *that* component contributes to real load-time RSS, on
//! the actual deserialization path, not a synthetic analog built separately.
//!
//! Usage: dump_snapshot_stripped <n_entities> <output_path> <flags>
//! flags: comma-separated subset of: attrs,arenas,index,nodes,bitmaps
//!   attrs    -- clear attribute_pairs_arena/attribute_set_values_arena
//!   arenas   -- clear the 5 per-entity SortedSetRef CSR arenas
//!   index    -- clear uuid_to_index/index_to_uuid
//!   nodes    -- replace `nodes` with an empty Vec (isolates its own struct overhead)
//!   bitmaps  -- clear action_to_policies/indexed_entity_types/entity_type_name_to_id
//!               and the 6 global aggregate RoaringBitmaps

use std::env;
use std::fs;

use arbor_bench::build_scenario;
use arbor_index_snapshot::PackagedSnapshot;
use arbor_types::IndexedNode;
use rapidhash::RapidHashMap;
use roaring::RoaringBitmap;

fn main() {
    let mut args = env::args().skip(1);
    let n: usize = args
        .next()
        .expect("usage: dump_snapshot_stripped <n_entities> <output_path> <flags>")
        .parse()
        .expect("n_entities must be a positive integer");
    let output_path = args.next().expect("missing output_path");
    let flags: Vec<String> = args
        .next()
        .unwrap_or_default()
        .split(',')
        .map(|s| s.to_string())
        .collect();
    let has = |f: &str| flags.iter().any(|x| x == f);

    let (mut snapshot, _fixtures) = build_scenario(n);

    if has("attrs") {
        snapshot.attribute_pairs_arena.clear();
        snapshot.attribute_set_values_arena.clear();
        for node in &mut snapshot.nodes {
            if let IndexedNode::Entity(e) = node {
                e.attributes = Default::default();
            }
        }
    }
    if has("arenas") {
        snapshot.ancestors_arena.clear();
        snapshot.principal_of_arena.clear();
        snapshot.resource_of_arena.clear();
        snapshot.effective_principal_arena.clear();
        snapshot.effective_resource_arena.clear();
        for node in &mut snapshot.nodes {
            if let IndexedNode::Entity(e) = node {
                e.ancestors = Default::default();
                e.principal_of_policies = None;
                e.resource_of_policies = None;
                e.effective_principal_policies = None;
                e.effective_resource_policies = None;
            }
        }
    }
    if has("index") {
        snapshot.uuid_to_index = RapidHashMap::default();
        snapshot.index_to_uuid.clear();
    }
    if has("nodes") {
        snapshot.nodes.clear();
    }
    if has("bitmaps") {
        snapshot.action_to_policies = RapidHashMap::default();
        snapshot.indexed_entity_types = RapidHashMap::default();
        snapshot.entity_type_name_to_id = RapidHashMap::default();
        snapshot.all_principal_policies = RoaringBitmap::new();
        snapshot.all_resource_policies = RoaringBitmap::new();
        snapshot.conditional_policies = RoaringBitmap::new();
        snapshot.forbidding_policies = RoaringBitmap::new();
        snapshot.descendant_principal_policies = RoaringBitmap::new();
        snapshot.descendant_resource_policies = RoaringBitmap::new();
    }

    let packaged = PackagedSnapshot::from_snapshot(snapshot, 0, 0).expect("package snapshot");
    let bytes = packaged.serialize().expect("serialize");
    fs::write(&output_path, &bytes).expect("write file");

    println!(
        "n_entities={n} flags={flags:?} file_mb={:.1}",
        bytes.len() as f64 / (1024.0 * 1024.0)
    );
}
