use rapidhash::RapidHashMap;
use roaring::RoaringBitmap;
use uuid::Uuid;
use serde::{Deserialize, Serialize};
use arbor_types::{
    AttributeNameId, EntityResolver, EntityTypeId, IndexedAttributeValue, IndexedEntity,
    IndexedEntityType, IndexedNode, IndexedPolicy, IndexedPolicyTarget, ArborError, ArborResult,
    SortedSetRef,
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
// Reserved-capacity Vec serde helper
//
// serde's derived `Vec<T>` deserializer deliberately ignores most of
// bincode's exact length prefix -- it calls `size_hint::cautious::<T>()`,
// which caps upfront reservation to guard against a corrupted/malicious
// length claiming billions of elements. For these fields the bincode
// stream's length is trustworthy (self-produced snapshot files within the
// indexer/authorizer trust boundary, same tradeoff already made by
// `rapid_hash_map_serde`), so reserving the full hint up front avoids the
// `RawVec` doubling reallocations that otherwise leave these Vecs at up to
// 2x their needed capacity -- confirmed via `malloc_history` on a 1M-entity
// snapshot load.
// ---------------------------------------------------------------------------

mod reserved_vec_serde {
    use serde::de::{Deserialize, Deserializer, SeqAccess, Visitor};
    use serde::ser::{Serialize, SerializeSeq, Serializer};
    use std::fmt;
    use std::marker::PhantomData;

    pub fn serialize<T, S>(vec: &[T], serializer: S) -> Result<S::Ok, S::Error>
    where
        T: Serialize,
        S: Serializer,
    {
        let mut seq = serializer.serialize_seq(Some(vec.len()))?;
        for item in vec {
            seq.serialize_element(item)?;
        }
        seq.end()
    }

    pub fn deserialize<'de, T, D>(deserializer: D) -> Result<Vec<T>, D::Error>
    where
        T: Deserialize<'de>,
        D: Deserializer<'de>,
    {
        struct ReservedVecVisitor<T>(PhantomData<T>);

        impl<'de, T> Visitor<'de> for ReservedVecVisitor<T>
        where
            T: Deserialize<'de>,
        {
            type Value = Vec<T>;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "a sequence")
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut vec = Vec::new();
                if let Some(hint) = seq.size_hint() {
                    vec.reserve(hint);
                }
                while let Some(item) = seq.next_element::<T>()? {
                    vec.push(item);
                }
                Ok(vec)
            }
        }

        deserializer.deserialize_seq(ReservedVecVisitor(PhantomData))
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

    #[serde(with = "reserved_vec_serde")]
    pub nodes: Vec<IndexedNode>,

    /// Shared arenas backing every `IndexedEntity` `SortedSetRef` field. One
    /// arena per field for the whole snapshot instead of a `RoaringBitmap`
    /// per entity — see `SortedSetRef` for why.
    #[serde(with = "reserved_vec_serde")]
    pub ancestors_arena: Vec<u32>,
    #[serde(with = "reserved_vec_serde")]
    pub principal_of_arena: Vec<u32>,
    #[serde(with = "reserved_vec_serde")]
    pub resource_of_arena: Vec<u32>,
    #[serde(with = "reserved_vec_serde")]
    pub effective_principal_arena: Vec<u32>,
    #[serde(with = "reserved_vec_serde")]
    pub effective_resource_arena: Vec<u32>,

    /// Backs every `IndexedEntity::attributes` and nested
    /// `IndexedAttributeValue::Object` -- named `(name, value)` pairs, sorted
    /// by name within each range.
    #[serde(with = "reserved_vec_serde")]
    pub attribute_pairs_arena: Vec<(AttributeNameId, IndexedAttributeValue)>,
    /// Backs every `IndexedAttributeValue::Set` -- unnamed elements. Separate
    /// from `attribute_pairs_arena` because `Set`'s contents have no names,
    /// not because the two are otherwise related.
    #[serde(with = "reserved_vec_serde")]
    pub attribute_set_values_arena: Vec<IndexedAttributeValue>,

    #[serde(with = "rapid_hash_map_serde")]
    pub action_to_policies: RapidHashMap<u32, RoaringBitmap>,
    /// Transitive descendant sets for `EntityWithDescendants` policy
    /// targets, keyed by target entity index and deduplicated -- policies
    /// sharing a target (e.g. many policies scoped to the same org root)
    /// share one entry instead of each carrying its own cloned copy.
    #[serde(with = "rapid_hash_map_serde")]
    pub descendants_by_target: RapidHashMap<u32, RoaringBitmap>,
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
            ancestors_arena: Vec::new(),
            principal_of_arena: Vec::new(),
            resource_of_arena: Vec::new(),
            effective_principal_arena: Vec::new(),
            effective_resource_arena: Vec::new(),
            attribute_pairs_arena: Vec::new(),
            attribute_set_values_arena: Vec::new(),
            indexed_entity_types: RapidHashMap::default(),
            entity_type_name_to_id: RapidHashMap::default(),
            all_principal_policies: RoaringBitmap::new(),
            all_resource_policies: RoaringBitmap::new(),
            conditional_policies: RoaringBitmap::new(),
            forbidding_policies: RoaringBitmap::new(),
            descendant_principal_policies: RoaringBitmap::new(),
            descendant_resource_policies: RoaringBitmap::new(),
            action_to_policies: RapidHashMap::default(),
            descendants_by_target: RapidHashMap::default(),
        }
    }

    pub fn get_entity(&self, idx: u32) -> Option<&IndexedEntity> {
        match self.nodes.get(idx as usize)? {
            IndexedNode::Entity(e) => Some(e),
            _ => None,
        }
    }

    /// Resolves an entity's `ancestors` `SortedSetRef` into its backing slice
    /// in `ancestors_arena`.
    pub fn ancestors_of(&self, idx: u32) -> Option<&[u32]> {
        let r = self.get_entity(idx)?.ancestors;
        Some(&self.ancestors_arena[r.offset as usize..(r.offset + r.len) as usize])
    }

    /// Resolves a `SortedSetRef` (an entity's own attributes, or a nested
    /// `Object`) into its `(name, value)` pairs.
    pub fn attribute_pairs(&self, range: SortedSetRef) -> &[(AttributeNameId, IndexedAttributeValue)] {
        &self.attribute_pairs_arena[range.offset as usize..(range.offset + range.len) as usize]
    }

    /// Resolves a `SortedSetRef` for an `IndexedAttributeValue::Set` into its
    /// (unnamed) elements.
    pub fn attribute_set_values(&self, range: SortedSetRef) -> &[IndexedAttributeValue] {
        &self.attribute_set_values_arena[range.offset as usize..(range.offset + range.len) as usize]
    }

    /// Resolves a `SortedSetRef` into its backing slice in `arena`. `None`
    /// (missing entity, or field not set) resolves to an empty slice —
    /// matches the old `Option<RoaringBitmap>` callers' `unwrap_or_default`
    /// behavior, since by the time these are called the entity's existence
    /// has already been checked by the caller.
    fn resolve<'a>(arena: &'a [u32], r: Option<SortedSetRef>) -> &'a [u32] {
        match r {
            Some(r) => &arena[r.offset as usize..(r.offset + r.len) as usize],
            None => &[],
        }
    }

    pub fn principal_of_policies_of(&self, idx: u32) -> &[u32] {
        let r = self.get_entity(idx).and_then(|e| e.principal_of_policies);
        Self::resolve(&self.principal_of_arena, r)
    }

    pub fn resource_of_policies_of(&self, idx: u32) -> &[u32] {
        let r = self.get_entity(idx).and_then(|e| e.resource_of_policies);
        Self::resolve(&self.resource_of_arena, r)
    }

    pub fn effective_principal_of(&self, idx: u32) -> &[u32] {
        let r = self.get_entity(idx).and_then(|e| e.effective_principal_policies);
        Self::resolve(&self.effective_principal_arena, r)
    }

    pub fn effective_resource_of(&self, idx: u32) -> &[u32] {
        let r = self.get_entity(idx).and_then(|e| e.effective_resource_policies);
        Self::resolve(&self.effective_resource_arena, r)
    }

    /// Two-pointer merge of two sorted slices, filtered by membership in
    /// `mask`. This is `check()`'s hot-path pattern: intersect two small
    /// per-entity sets first, then only test the (few) survivors against a
    /// larger `RoaringBitmap` like `action_to_policies`, instead of doing two
    /// full `RoaringBitmap` ANDs.
    ///
    /// Returns a plain `Vec<u32>`, not a `RoaringBitmap` — the result is fed
    /// straight into `split_policy_map_for_authorization`, which itself only
    /// needs to iterate it; materializing a `RoaringBitmap` here would pay
    /// the same small-object allocation cost this whole representation
    /// change was meant to eliminate.
    pub fn merge_and_filter_sorted(a: &[u32], b: &[u32], mask: &RoaringBitmap) -> Vec<u32> {
        let mut result = Vec::new();
        let (mut i, mut j) = (0, 0);
        while i < a.len() && j < b.len() {
            match a[i].cmp(&b[j]) {
                std::cmp::Ordering::Less => i += 1,
                std::cmp::Ordering::Greater => j += 1,
                std::cmp::Ordering::Equal => {
                    if mask.contains(a[i]) {
                        result.push(a[i]);
                    }
                    i += 1;
                    j += 1;
                }
            }
        }
        result
    }

    /// Filters a single sorted slice down to elements also present in `mask`.
    pub fn filter_sorted_by_mask(sorted: &[u32], mask: &RoaringBitmap) -> Vec<u32> {
        sorted.iter().copied().filter(|x| mask.contains(*x)).collect()
    }

    /// Two-pointer intersection of two small sorted slices (no larger mask
    /// involved — both operands are per-entity/per-check-call sized).
    pub fn intersect_sorted(a: &[u32], b: &[u32]) -> Vec<u32> {
        let mut result = Vec::new();
        let (mut i, mut j) = (0, 0);
        while i < a.len() && j < b.len() {
            match a[i].cmp(&b[j]) {
                std::cmp::Ordering::Less => i += 1,
                std::cmp::Ordering::Greater => j += 1,
                std::cmp::Ordering::Equal => {
                    result.push(a[i]);
                    i += 1;
                    j += 1;
                }
            }
        }
        result
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
        policies: &[u32],
        entity_type_id: EntityTypeId,
        side: PolicySide,
    ) -> ArborResult<RoaringBitmap> {
        let et = self.indexed_entity_types.get(&entity_type_id)
            .ok_or_else(|| ArborError::EntityNotFound(format!("Entity type not found {:?}", entity_type_id)))?;
        let mut acc = RoaringBitmap::new();
        for &policy_idx in policies {
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
                IndexedPolicyTarget::EntityWithDescendants(idx) => {
                    if let Some(desc) = self.descendants_by_target.get(&idx) {
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
        mask: &[u32],
        side: PolicySide,
    ) -> ArborResult<Vec<u32>> {
        self.get_entity(entity_idx)
            .ok_or_else(|| ArborError::EntityNotFound(format!("Entity not found {}", entity_idx)))?;
        let effective = match side {
            PolicySide::Principal => self.effective_principal_of(entity_idx),
            PolicySide::Resource => self.effective_resource_of(entity_idx),
        };
        Ok(Self::intersect_sorted(effective, mask))
    }

    pub fn get_entity_type_id_by_name(&self, name: &str) -> Option<EntityTypeId> {
        self.entity_type_name_to_id.get(name).copied()
    }

    /// Classifies each policy index in `policy_bitmap` (a small, per-check
    /// result set) into one of four buckets by probing the large global
    /// `conditional_policies`/`forbidding_policies` masks -- one pass over
    /// the small input, two `.contains()` probes each, instead of four
    /// RoaringBitmap AND/SUB operations against those large masks. Preserves
    /// `policy_bitmap`'s sort order in each output bucket.
    pub fn split_policy_map_for_authorization(
        &self,
        policy_bitmap: &[u32],
    ) -> (Vec<u32>, Vec<u32>, Vec<u32>, Vec<u32>) {
        let mut unconditional_forbidding = Vec::new();
        let mut conditional_forbidding = Vec::new();
        let mut unconditional_permitting = Vec::new();
        let mut conditional_permitting = Vec::new();

        for &p in policy_bitmap {
            let conditional = self.conditional_policies.contains(p);
            let forbidding = self.forbidding_policies.contains(p);
            match (conditional, forbidding) {
                (false, true) => unconditional_forbidding.push(p),
                (true, true) => conditional_forbidding.push(p),
                (false, false) => unconditional_permitting.push(p),
                (true, false) => conditional_permitting.push(p),
            }
        }

        (unconditional_forbidding, conditional_forbidding, unconditional_permitting, conditional_permitting)
    }
}

impl EntityResolver for Snapshot {
    fn get_entity(&self, index: u32) -> Option<&IndexedEntity> {
        self.get_entity(index)
    }

    fn ancestors_of(&self, index: u32) -> Option<&[u32]> {
        self.ancestors_of(index)
    }

    fn attribute_pairs(&self, range: SortedSetRef) -> &[(AttributeNameId, IndexedAttributeValue)] {
        self.attribute_pairs(range)
    }

    fn attribute_set_values(&self, range: SortedSetRef) -> &[IndexedAttributeValue] {
        self.attribute_set_values(range)
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
