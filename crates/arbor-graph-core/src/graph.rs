use std::collections::HashSet;
use rapidhash::{HashMapExt, RapidHashMap};
use smallvec::SmallVec;
use uuid::Uuid;
use arbor_types::EntityTypeId;
use crate::types;

pub struct Graph {
    pub nodes: Vec<types::NodeType>,
    pub free_nodes: SmallVec<u32, 8>,
    pub next_index: u32,

    pub parents: RapidHashMap<u32, HashSet<u32>>,
    pub children: RapidHashMap<u32, HashSet<u32>>,
    
    pub uuid_to_index: RapidHashMap<Uuid, u32>,
    pub entity_type_names: RapidHashMap<EntityTypeId, String>,
}

impl Default for Graph {
    fn default() -> Self {
        Self::new()
    }
}

impl Graph {
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            free_nodes: SmallVec::new(),
            next_index: 0,

            parents: RapidHashMap::new(),
            children: RapidHashMap::new(),

            uuid_to_index: RapidHashMap::new(),
            entity_type_names: RapidHashMap::new(),
        }
    }
}