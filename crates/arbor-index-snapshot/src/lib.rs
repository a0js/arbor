use std::ops::Sub;
use rapidhash::RapidHashMap;
use roaring::RoaringBitmap;
use uuid::Uuid;
use serde::{Deserialize, Serialize};
use arbor_types::{
    EntityResolver, EntityTypeId, IndexedEntity, IndexedEntityType, IndexedNode, IndexedPolicy,
    IndexedPolicyTarget, ArborError, ArborResult,
};

// ---------------------------------------------------------------------------
// PolicySide
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicySide {
    Principal,
    Resource,
}

// ---------------------------------------------------------------------------
// RapidHashMap serde helper
//
// `RapidHashMap<K, V>` does not implement serde traits because it uses a
// non-standard hasher.  We serialise it as a sorted `Vec<(K, V)>` to keep
// output deterministic and reconstruct it on deserialise.
// ---------------------------------------------------------------------------

mod rapid_hash_map_serde {
    use rapidhash::RapidHashMap;
    use serde::de::{Deserialize, Deserializer, SeqAccess, Visitor};
    use serde::ser::{Serialize, SerializeSeq, Serializer};
    use std::fmt;
    use std::hash::Hash;
    use std::marker::PhantomData;

    pub fn serialize<K, V, S>(map: &RapidHashMap<K, V>, serializer: S) -> Result<S::Ok, S::Error>
    where
        K: Serialize + Ord,
        V: Serialize,
        S: Serializer,
    {
        let mut pairs: Vec<(&K, &V)> = map.iter().collect();
        pairs.sort_by_key(|(k, _)| *k);
        let mut seq = serializer.serialize_seq(Some(pairs.len()))?;
        for (k, v) in pairs {
            seq.serialize_element(&(k, v))?;
        }
        seq.end()
    }

    pub fn deserialize<'de, K, V, D>(deserializer: D) -> Result<RapidHashMap<K, V>, D::Error>
    where
        K: Deserialize<'de> + Eq + Hash,
        V: Deserialize<'de>,
        D: Deserializer<'de>,
    {
        struct PairVisitor<K, V>(PhantomData<(K, V)>);

        impl<'de, K, V> Visitor<'de> for PairVisitor<K, V>
        where
            K: Deserialize<'de> + Eq + Hash,
            V: Deserialize<'de>,
        {
            type Value = RapidHashMap<K, V>;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "a sequence of (key, value) pairs")
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut map: RapidHashMap<K, V> = RapidHashMap::default();
                if let Some(hint) = seq.size_hint() {
                    map.reserve(hint);
                }
                while let Some((k, v)) = seq.next_element::<(K, V)>()? {
                    map.insert(k, v);
                }
                Ok(map)
            }
        }

        deserializer.deserialize_seq(PairVisitor(PhantomData))
    }
}

// ---------------------------------------------------------------------------
// Snapshot
// ---------------------------------------------------------------------------

/// Serialisable form of the `Snapshot`.
///
/// The struct is annotated with `#[serde(with = ...)]` on every `RapidHashMap`
/// field so that the standard bincode/serde pipeline works without any manual
/// `Serialize`/`Deserialize` implementations.
#[derive(Serialize, Deserialize)]
pub struct Snapshot {
    #[serde(with = "rapid_hash_map_serde")]
    pub uuid_to_index: RapidHashMap<Uuid, u32>,
    pub index_to_uuid: Vec<Option<Uuid>>,

    pub nodes: Vec<IndexedNode>,

    #[serde(with = "rapid_hash_map_serde")]
    pub action_to_policies: RapidHashMap<u32, RoaringBitmap>,
    #[serde(with = "rapid_hash_map_serde")]
    pub indexed_entity_types: RapidHashMap<EntityTypeId, IndexedEntityType>,

    #[serde(with = "rapid_hash_map_serde")]
    pub entity_type_name_to_id: RapidHashMap<String, EntityTypeId>,

    pub all_principal_policies: RoaringBitmap,
    pub all_resource_policies: RoaringBitmap,
    pub conditional_policies: RoaringBitmap,
    pub forbidding_policies: RoaringBitmap,
    pub descendant_principal_policies: RoaringBitmap,
    pub descendant_resource_policies: RoaringBitmap,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self::new()
    }
}

impl Snapshot {
    pub fn new() -> Self {
        Self {
            uuid_to_index: RapidHashMap::default(),
            index_to_uuid: Vec::new(),
            nodes: Vec::new(),
            indexed_entity_types: RapidHashMap::default(),
            entity_type_name_to_id: RapidHashMap::default(),
            all_principal_policies: RoaringBitmap::new(),
            all_resource_policies: RoaringBitmap::new(),
            conditional_policies: RoaringBitmap::new(),
            forbidding_policies: RoaringBitmap::new(),
            descendant_principal_policies: RoaringBitmap::new(),
            descendant_resource_policies: RoaringBitmap::new(),
            action_to_policies: RapidHashMap::default(),
        }
    }

    pub fn get_entity(&self, idx: u32) -> Option<&IndexedEntity> {
        match self.nodes.get(idx as usize)? {
            IndexedNode::Entity(e) => Some(e),
            _ => None,
        }
    }

    pub fn get_policy(&self, idx: u32) -> Option<&IndexedPolicy> {
        match self.nodes.get(idx as usize)? {
            IndexedNode::Policy(p) => Some(p),
            _ => None,
        }
    }

    pub fn get_policies_for_action(&self, action_idx: u32) -> ArborResult<&RoaringBitmap> {
        self.action_to_policies
            .get(&action_idx)
            .ok_or_else(|| ArborError::EntityNotFound(format!("Action not found {}", action_idx)))
    }

    pub fn get_entities_of_type_for_policies(
        &self,
        policies: &RoaringBitmap,
        entity_type_id: EntityTypeId,
        side: PolicySide,
    ) -> ArborResult<RoaringBitmap> {
        let et = self.indexed_entity_types.get(&entity_type_id)
            .ok_or_else(|| ArborError::EntityNotFound(format!("Entity type not found {:?}", entity_type_id)))?;
        let mut acc = RoaringBitmap::new();
        for policy_idx in policies.iter() {
            let policy = self.get_policy(policy_idx)
                .ok_or_else(|| ArborError::EntityNotFound(format!("Policy not found {}", policy_idx)))?;
            let target = match side {
                PolicySide::Principal => policy.principal_target,
                PolicySide::Resource => policy.resource_target,
            };
            match target {
                IndexedPolicyTarget::Entity(idx) => {
                    if et.nodes_of_type.contains(idx) { acc.insert(idx); }
                }
                IndexedPolicyTarget::EntityWithDescendants(_idx) => {
                    let descendants = match side {
                        PolicySide::Principal => policy.principal_descendants.as_ref(),
                        PolicySide::Resource => policy.resource_descendants.as_ref(),
                    };
                    if let Some(desc) = descendants {
                        acc |= &et.nodes_of_type & desc;
                    }
                }
                IndexedPolicyTarget::EntityType(tid) => {
                    if tid == entity_type_id { return Ok(et.nodes_of_type.clone()); }
                }
                IndexedPolicyTarget::All => return Ok(et.nodes_of_type.clone()),
            }
        }
        Ok(acc)
    }

    /// Returns the intersection of `mask` with the precomputed effective policies for
    /// `entity_idx` on `side`.
    ///
    /// Uses the `effective_principal_policies` / `effective_resource_policies` fields
    /// computed at index time, so this is a single bitmap intersection.
    pub fn get_effective_policies_intersected(
        &self,
        entity_idx: u32,
        mask: &RoaringBitmap,
        side: PolicySide,
    ) -> ArborResult<RoaringBitmap> {
        let entity = self.get_entity(entity_idx)
            .ok_or_else(|| ArborError::EntityNotFound(format!("Entity not found {}", entity_idx)))?;
        let effective = match side {
            PolicySide::Principal => entity.effective_principal_policies.as_ref(),
            PolicySide::Resource => entity.effective_resource_policies.as_ref(),
        };
        Ok(effective.map(|e| e & mask).unwrap_or_default())
    }

    pub fn get_actions_for_policy(&self, policy_idx: u32) -> ArborResult<&RoaringBitmap> {
        self.get_policy(policy_idx)
            .map(|p| &p.actions)
            .ok_or_else(|| ArborError::EntityNotFound(format!("Policy not found {}", policy_idx)))
    }

    pub fn get_entity_type_id_by_name(&self, name: &str) -> Option<EntityTypeId> {
        self.entity_type_name_to_id.get(name).copied()
    }

    pub fn split_policy_map_for_authorization(
        &self,
        policy_bitmap: &RoaringBitmap,
    ) -> (RoaringBitmap, RoaringBitmap, RoaringBitmap, RoaringBitmap) {
        let conditional = policy_bitmap & &self.conditional_policies;
        let unconditional = policy_bitmap.sub(&self.conditional_policies);

        let unconditional_forbidding = &unconditional & &self.forbidding_policies;
        let conditional_forbidding   = &conditional  & &self.forbidding_policies;
        let unconditional_permitting = unconditional.sub(&self.forbidding_policies);
        let conditional_permitting   = conditional.sub(&self.forbidding_policies);

        (unconditional_forbidding, conditional_forbidding, unconditional_permitting, conditional_permitting)
    }
}

impl EntityResolver for Snapshot {
    fn get_entity(&self, index: u32) -> Option<&IndexedEntity> {
        self.get_entity(index)
    }
}

// ---------------------------------------------------------------------------
// Serialization error
// ---------------------------------------------------------------------------

/// Errors that can occur during snapshot serialization or deserialization.
#[derive(Debug, thiserror::Error)]
pub enum SerializationError {
    #[error("bincode error: {0}")]
    Bincode(#[from] bincode::Error),

    #[error("compression error: {0}")]
    Compression(String),

    #[error("decompression error: {0}")]
    Decompression(String),
}

// ---------------------------------------------------------------------------
// SnapshotMetadata
// ---------------------------------------------------------------------------

/// Summary statistics embedded in every packaged snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotMetadata {
    /// Number of entities in the snapshot.
    pub entity_count: u32,
    /// Number of policies in the snapshot.
    pub policy_count: u32,
    /// Number of distinct actions referenced by policies.
    pub action_count: u32,
    /// Wall-clock time taken to generate the snapshot, in milliseconds.
    pub generation_duration_ms: u64,
}

// ---------------------------------------------------------------------------
// PackagedSnapshot and internal wire format
// ---------------------------------------------------------------------------

/// A snapshot together with its provenance metadata.
///
/// Use [`PackagedSnapshot::from_snapshot`] to produce one from a raw
/// [`Snapshot`], [`PackagedSnapshot::serialize`] to write it to bytes, and
/// [`PackagedSnapshot::deserialize`] to read it back.
#[derive(Serialize, Deserialize)]
pub struct PackagedSnapshot {
    /// Monotonically increasing generation counter set by the indexer.
    pub version: u64,
    /// Unix timestamp (milliseconds) at which the snapshot was packaged.
    pub created_at_ms: i64,
    /// Summary statistics for the snapshot contents.
    pub metadata: SnapshotMetadata,
    /// lz4-compressed bincode encoding of the [`Snapshot`].
    compressed_data: Vec<u8>,
}

impl PackagedSnapshot {
    /// Build a [`PackagedSnapshot`] from a raw [`Snapshot`].
    ///
    /// # Errors
    ///
    /// Returns [`SerializationError::Bincode`] if bincode encoding fails.
    pub fn from_snapshot(
        snapshot: Snapshot,
        version: u64,
        generation_duration_ms: u64,
    ) -> Result<Self, SerializationError> {
        let (entity_count, policy_count) = snapshot.nodes.iter().fold((0u32, 0u32), |(e, p), node| {
            match node {
                IndexedNode::Entity(_) => (e + 1, p),
                IndexedNode::Policy(_) => (e, p + 1),
                IndexedNode::Other => (e, p),
            }
        });
        let action_count = snapshot.action_to_policies.len() as u32;

        let metadata = SnapshotMetadata {
            entity_count,
            policy_count,
            action_count,
            generation_duration_ms,
        };

        let created_at_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);

        let compressed_data = lz4_flex::compress_prepend_size(&bincode::serialize(&snapshot)?);

        Ok(Self { version, created_at_ms, metadata, compressed_data })
    }

    /// Encode this packaged snapshot to bytes.
    ///
    /// # Errors
    ///
    /// Returns [`SerializationError::Bincode`] if bincode encoding fails.
    pub fn serialize(&self) -> Result<Vec<u8>, SerializationError> {
        Ok(bincode::serialize(self)?)
    }

    /// Decode a packaged snapshot from bytes produced by [`PackagedSnapshot::serialize`].
    ///
    /// # Errors
    ///
    /// - [`SerializationError::Bincode`] on decode failure.
    /// - [`SerializationError::Decompression`] if lz4 decompression fails.
    pub fn deserialize(bytes: &[u8]) -> Result<Self, SerializationError> {
        Ok(bincode::deserialize(bytes)?)
    }

    /// Decompress and return the inner [`Snapshot`].
    ///
    /// # Errors
    ///
    /// - [`SerializationError::Decompression`] if lz4 decompression fails.
    /// - [`SerializationError::Bincode`] if bincode decoding fails.
    pub fn into_snapshot(self) -> Result<Snapshot, SerializationError> {
        let raw = lz4_flex::decompress_size_prepended(&self.compressed_data)
            .map_err(|e| SerializationError::Decompression(e.to_string()))?;
        Ok(bincode::deserialize(&raw)?)
    }
}
