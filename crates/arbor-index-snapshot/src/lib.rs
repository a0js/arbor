use rapidhash::RapidHashMap;
use roaring::RoaringBitmap;
use uuid::Uuid;
use serde::{Deserialize, Serialize};
use rkyv::{Archive, Deserialize as RkyvDeserialize, Serialize as RkyvSerialize};
use rkyv::with::{DeserializeWith, Identity, MapKV};
use arbor_types::{
    AttributeNameId, AttributeValueView, EntityResolver, EntityTypeId, IndexedAttributeValue, IndexedEntity,
    IndexedEntityType, IndexedNode, IndexedPolicy, IndexedPolicyTarget, ArborError, ArborResult,
    SortedSetRef,
};
use arbor_types::rkyv_with::RoaringAsBytes;

// ---------------------------------------------------------------------------
// PolicySide
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicySide {
    Principal,
    Resource,
}

// ---------------------------------------------------------------------------
// Snapshot
// ---------------------------------------------------------------------------

#[derive(Archive, RkyvSerialize, RkyvDeserialize)]
pub struct Snapshot {
    pub uuid_to_index: RapidHashMap<Uuid, u32>,
    pub index_to_uuid: Vec<Option<Uuid>>,

    pub nodes: Vec<IndexedNode>,

    /// Shared arenas backing every `IndexedEntity` `SortedSetRef` field. One
    /// arena per field for the whole snapshot instead of a `RoaringBitmap`
    /// per entity — see `SortedSetRef` for why.
    pub ancestors_arena: Vec<u32>,
    pub principal_of_arena: Vec<u32>,
    pub resource_of_arena: Vec<u32>,
    pub effective_principal_arena: Vec<u32>,
    pub effective_resource_arena: Vec<u32>,

    /// Backs every `IndexedEntity::attributes` and nested
    /// `IndexedAttributeValue::Object` -- named `(name, value)` pairs, sorted
    /// by name within each range.
    pub attribute_pairs_arena: Vec<(AttributeNameId, IndexedAttributeValue)>,
    /// Backs every `IndexedAttributeValue::Set` -- unnamed elements. Separate
    /// from `attribute_pairs_arena` because `Set`'s contents have no names,
    /// not because the two are otherwise related.
    pub attribute_set_values_arena: Vec<IndexedAttributeValue>,

    #[rkyv(with = MapKV<Identity, RoaringAsBytes>)]
    pub action_to_policies: RapidHashMap<u32, RoaringBitmap>,
    /// Transitive descendant sets for `EntityWithDescendants` policy
    /// targets, keyed by target entity index and deduplicated -- policies
    /// sharing a target (e.g. many policies scoped to the same org root)
    /// share one entry instead of each carrying its own cloned copy.
    #[rkyv(with = MapKV<Identity, RoaringAsBytes>)]
    pub descendants_by_target: RapidHashMap<u32, RoaringBitmap>,
    pub indexed_entity_types: RapidHashMap<EntityTypeId, IndexedEntityType>,

    pub entity_type_name_to_id: RapidHashMap<String, EntityTypeId>,

    #[rkyv(with = RoaringAsBytes)]
    pub all_principal_policies: RoaringBitmap,
    #[rkyv(with = RoaringAsBytes)]
    pub all_resource_policies: RoaringBitmap,
    #[rkyv(with = RoaringAsBytes)]
    pub conditional_policies: RoaringBitmap,
    #[rkyv(with = RoaringAsBytes)]
    pub forbidding_policies: RoaringBitmap,
    #[rkyv(with = RoaringAsBytes)]
    pub descendant_principal_policies: RoaringBitmap,
    #[rkyv(with = RoaringAsBytes)]
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

    fn resolve_attribute_path(&self, base: SortedSetRef, path: &[AttributeNameId]) -> Option<AttributeValueView<'_>> {
        if path.is_empty() {
            return None;
        }

        let pairs = self.attribute_pairs(base);
        let mut current = pairs
            .binary_search_by_key(&path[0], |(k, _)| *k)
            .ok()
            .map(|i| &pairs[i].1)?;

        for &name in &path[1..] {
            match current {
                IndexedAttributeValue::Object(nested) => {
                    let nested_pairs = self.attribute_pairs(*nested);
                    current = nested_pairs
                        .binary_search_by_key(&name, |(k, _)| *k)
                        .ok()
                        .map(|i| &nested_pairs[i].1)?;
                }
                _ => return None,
            }
        }

        Some(current.as_view())
    }

    fn attribute_set_values(&self, range: SortedSetRef) -> Vec<AttributeValueView<'_>> {
        self.attribute_set_values(range).iter().map(|v| v.as_view()).collect()
    }

    fn attribute_pairs_view(&self, range: SortedSetRef) -> Vec<(AttributeNameId, AttributeValueView<'_>)> {
        self.attribute_pairs(range).iter().map(|(name, v)| (*name, v.as_view())).collect()
    }
}

// ---------------------------------------------------------------------------
// SnapshotOps
//
// Everything `AuthorizerEngine::check`/`list_entities` need beyond
// `EntityResolver`, so both the in-memory `Snapshot` (used while building,
// via `AuthorizerEngine::from_snapshot`) and the rkyv-backed `RkyvSnapshot`
// (the production read path) can sit behind the same `Arc<dyn SnapshotOps>` in
// `AuthorizerEngine`. `merge_and_filter_sorted`/`filter_sorted_by_mask`/
// `intersect_sorted` aren't here -- they're pure functions on slices that
// never touch `self`, already callable via `Snapshot::` regardless of which
// backing store is active.
// ---------------------------------------------------------------------------

pub trait SnapshotOps: EntityResolver {
    fn get_policy(&self, idx: u32) -> Option<&IndexedPolicy>;
    fn get_policies_for_action(&self, action_idx: u32) -> ArborResult<&RoaringBitmap>;
    fn effective_principal_of(&self, idx: u32) -> &[u32];
    fn effective_resource_of(&self, idx: u32) -> &[u32];
    fn split_policy_map_for_authorization(
        &self,
        policy_bitmap: &[u32],
    ) -> (Vec<u32>, Vec<u32>, Vec<u32>, Vec<u32>);
    fn get_entities_of_type_for_policies(
        &self,
        policies: &[u32],
        entity_type_id: EntityTypeId,
        side: PolicySide,
    ) -> ArborResult<RoaringBitmap>;
    fn get_effective_policies_intersected(
        &self,
        entity_idx: u32,
        mask: &[u32],
        side: PolicySide,
    ) -> ArborResult<Vec<u32>>;
    /// Upcast to `&dyn EntityResolver` for `EvaluationContext::new`, which
    /// takes a trait object -- needed until/unless trait upcasting coercion
    /// covers this case directly.
    fn as_entity_resolver(&self) -> &dyn EntityResolver;

    /// Resolve an entity/policy's client-facing UUID to its snapshot index
    /// -- used by the gRPC service layer's request-parsing, not by
    /// `check()`/`list_entities()` themselves.
    fn uuid_to_index(&self, uuid: &Uuid) -> Option<u32>;
    /// Resolve a snapshot index back to its UUID, for building responses.
    fn index_to_uuid(&self, idx: u32) -> Option<Uuid>;
    fn get_entity_type_id_by_name(&self, name: &str) -> Option<EntityTypeId>;
}

impl SnapshotOps for Snapshot {
    fn get_policy(&self, idx: u32) -> Option<&IndexedPolicy> {
        Snapshot::get_policy(self, idx)
    }
    fn get_policies_for_action(&self, action_idx: u32) -> ArborResult<&RoaringBitmap> {
        Snapshot::get_policies_for_action(self, action_idx)
    }
    fn effective_principal_of(&self, idx: u32) -> &[u32] {
        Snapshot::effective_principal_of(self, idx)
    }
    fn effective_resource_of(&self, idx: u32) -> &[u32] {
        Snapshot::effective_resource_of(self, idx)
    }
    fn split_policy_map_for_authorization(
        &self,
        policy_bitmap: &[u32],
    ) -> (Vec<u32>, Vec<u32>, Vec<u32>, Vec<u32>) {
        Snapshot::split_policy_map_for_authorization(self, policy_bitmap)
    }
    fn get_entities_of_type_for_policies(
        &self,
        policies: &[u32],
        entity_type_id: EntityTypeId,
        side: PolicySide,
    ) -> ArborResult<RoaringBitmap> {
        Snapshot::get_entities_of_type_for_policies(self, policies, entity_type_id, side)
    }
    fn get_effective_policies_intersected(
        &self,
        entity_idx: u32,
        mask: &[u32],
        side: PolicySide,
    ) -> ArborResult<Vec<u32>> {
        Snapshot::get_effective_policies_intersected(self, entity_idx, mask, side)
    }
    fn as_entity_resolver(&self) -> &dyn EntityResolver {
        self
    }
    fn uuid_to_index(&self, uuid: &Uuid) -> Option<u32> {
        self.uuid_to_index.get(uuid).copied()
    }
    fn index_to_uuid(&self, idx: u32) -> Option<Uuid> {
        self.index_to_uuid.get(idx as usize).copied().flatten()
    }
    fn get_entity_type_id_by_name(&self, name: &str) -> Option<EntityTypeId> {
        Snapshot::get_entity_type_id_by_name(self, name)
    }
}

// ---------------------------------------------------------------------------
// RkyvSnapshot
//
// rkyv-backed reader: lz4-decompress into one owned buffer, `rkyv::access`
// once (validated), then split into two halves:
//
//   - Bulk-materialized (nodes, the three u32 CSR arenas the query path
//     reads, and the RoaringBitmap-backed fields it reads): each is ONE
//     contiguous allocation --
//     cheap, not the allocation-COUNT problem this format change targets.
//     `nodes`/`IndexedEntity` have no heap data of their own (attributes
//     resolve via SortedSetRef into the arenas, not embedded), and
//     `IndexedPolicy` only pays a real allocation for the (small, bounded)
//     subset of policies that are conditional. Letting the derive-generated
//     `Deserialize` impls do this (`rkyv::deserialize`) rather than
//     hand-rolling field-by-field conversion.
//
//   - Left on the raw archive (attribute_pairs_arena/attribute_set_values_arena):
//     these hold `IndexedAttributeValue::String`, the actual 500K-allocation
//     source this whole investigation was about -- reads go through
//     `self.archived()` and `.as_view()`, zero-copy.
// ---------------------------------------------------------------------------

/// A `&ArchivedSnapshot` with an explicit lifetime parameter, as `self_cell`
/// requires for the "dependent" (borrowing) half of an owner+borrow pair --
/// `ArchivedSnapshot` itself has no lifetime of its own (it's a plain sized
/// type you reach via a reference), so this thin wrapper is what actually
/// carries the borrow `self_cell` tracks.
struct ArchivedSnapshotRef<'a>(&'a ArchivedSnapshot);

self_cell::self_cell!(
    /// Owns the decompressed archive bytes and a validated `&ArchivedSnapshot`
    /// borrowing from them, safely -- `self_cell` generates the small amount
    /// of `unsafe` this requires internally (reviewed, minimal, no proc-macros),
    /// so `RkyvSnapshot` itself contains none.
    struct ArchiveCell {
        owner: Box<[u8]>,
        #[covariant]
        dependent: ArchivedSnapshotRef,
    }
);

pub struct RkyvSnapshot {
    archive: ArchiveCell,

    nodes: Vec<IndexedNode>,
    ancestors_arena: Vec<u32>,
    effective_principal_arena: Vec<u32>,
    effective_resource_arena: Vec<u32>,

    action_to_policies: RapidHashMap<u32, RoaringBitmap>,
    descendants_by_target: RapidHashMap<u32, RoaringBitmap>,
    indexed_entity_types: RapidHashMap<EntityTypeId, IndexedEntityType>,
    conditional_policies: RoaringBitmap,
    forbidding_policies: RoaringBitmap,

    uuid_to_index: RapidHashMap<Uuid, u32>,
    index_to_uuid: Vec<Option<Uuid>>,
    entity_type_name_to_id: RapidHashMap<String, EntityTypeId>,
}

fn rkyv_err(e: impl std::fmt::Display) -> SerializationError {
    SerializationError::Rkyv(e.to_string())
}

impl RkyvSnapshot {
    /// Loads a file produced by [`RkyvPackagedSnapshot::serialize`] (the
    /// real, versioned production format).
    pub fn load(path: &std::path::Path) -> Result<Self, SerializationError> {
        let bytes = std::fs::read(path)?;
        let packaged = RkyvPackagedSnapshot::deserialize(&bytes)?;
        Self::from_compressed_bytes(packaged.into_compressed_data())
    }

    /// Builds a `RkyvSnapshot` directly from lz4-compressed rkyv archive
    /// bytes (the payload inside a [`RkyvPackagedSnapshot`], or a raw
    /// archive produced without the metadata wrapper -- e.g. by benchmarks
    /// isolating archive-access performance specifically).
    pub fn from_compressed_bytes(compressed: Vec<u8>) -> Result<Self, SerializationError> {
        let raw = lz4_flex::decompress_size_prepended(&compressed)
            .map_err(|e| SerializationError::Decompression(e.to_string()))?;
        let bytes: Box<[u8]> = raw.into_boxed_slice();

        let archive = ArchiveCell::try_new(bytes, |b| {
            rkyv::access::<ArchivedSnapshot, rkyv::rancor::Error>(b)
                .map(ArchivedSnapshotRef)
                .map_err(rkyv_err)
        })?;
        let archived_ref = archive.borrow_dependent().0;

        let nodes: Vec<IndexedNode> = archived_ref
            .nodes
            .iter()
            .map(|n| rkyv::deserialize::<IndexedNode, rkyv::rancor::Error>(n).expect("deserialize IndexedNode"))
            .collect();
        let ancestors_arena = rkyv::deserialize::<Vec<u32>, rkyv::rancor::Error>(&archived_ref.ancestors_arena)
            .expect("deserialize ancestors_arena");
        let effective_principal_arena =
            rkyv::deserialize::<Vec<u32>, rkyv::rancor::Error>(&archived_ref.effective_principal_arena)
                .expect("deserialize effective_principal_arena");
        let effective_resource_arena =
            rkyv::deserialize::<Vec<u32>, rkyv::rancor::Error>(&archived_ref.effective_resource_arena)
                .expect("deserialize effective_resource_arena");

        // `rkyv::deserialize` only routes through a `with`-wrapper when it's
        // deserializing the *enclosing* struct (the derive macro wires that
        // up); deserializing one of these fields in isolation needs the
        // wrapper's `deserialize_with` called directly.
        let action_to_policies = MapKV::<Identity, RoaringAsBytes>::deserialize_with(
            &archived_ref.action_to_policies,
            rkyv::rancor::Strategy::<_, rkyv::rancor::Error>::wrap(&mut ()),
        )
        .expect("deserialize action_to_policies");
        let descendants_by_target = MapKV::<Identity, RoaringAsBytes>::deserialize_with(
            &archived_ref.descendants_by_target,
            rkyv::rancor::Strategy::<_, rkyv::rancor::Error>::wrap(&mut ()),
        )
        .expect("deserialize descendants_by_target");
        let indexed_entity_types = rkyv::deserialize::<RapidHashMap<EntityTypeId, IndexedEntityType>, rkyv::rancor::Error>(
            &archived_ref.indexed_entity_types,
        )
        .expect("deserialize indexed_entity_types");
        let conditional_policies = RoaringAsBytes::deserialize_with(
            &archived_ref.conditional_policies,
            rkyv::rancor::Strategy::<_, rkyv::rancor::Error>::wrap(&mut ()),
        )
        .expect("deserialize conditional_policies");
        let forbidding_policies = RoaringAsBytes::deserialize_with(
            &archived_ref.forbidding_policies,
            rkyv::rancor::Strategy::<_, rkyv::rancor::Error>::wrap(&mut ()),
        )
        .expect("deserialize forbidding_policies");

        let uuid_to_index = rkyv::deserialize::<RapidHashMap<Uuid, u32>, rkyv::rancor::Error>(&archived_ref.uuid_to_index)
            .expect("deserialize uuid_to_index");
        let index_to_uuid = rkyv::deserialize::<Vec<Option<Uuid>>, rkyv::rancor::Error>(&archived_ref.index_to_uuid)
            .expect("deserialize index_to_uuid");
        let entity_type_name_to_id =
            rkyv::deserialize::<RapidHashMap<String, EntityTypeId>, rkyv::rancor::Error>(&archived_ref.entity_type_name_to_id)
                .expect("deserialize entity_type_name_to_id");

        Ok(Self {
            archive,
            nodes,
            ancestors_arena,
            effective_principal_arena,
            effective_resource_arena,
            action_to_policies,
            descendants_by_target,
            indexed_entity_types,
            conditional_policies,
            forbidding_policies,
            uuid_to_index,
            index_to_uuid,
            entity_type_name_to_id,
        })
    }

    fn archived(&self) -> &ArchivedSnapshot {
        self.archive.borrow_dependent().0
    }

    fn get_entity(&self, idx: u32) -> Option<&IndexedEntity> {
        match self.nodes.get(idx as usize)? {
            IndexedNode::Entity(e) => Some(e),
            _ => None,
        }
    }

    fn get_policy(&self, idx: u32) -> Option<&IndexedPolicy> {
        match self.nodes.get(idx as usize)? {
            IndexedNode::Policy(p) => Some(p),
            _ => None,
        }
    }

    fn resolve<'a>(arena: &'a [u32], r: Option<SortedSetRef>) -> &'a [u32] {
        match r {
            Some(r) => &arena[r.offset as usize..(r.offset + r.len) as usize],
            None => &[],
        }
    }
}

impl EntityResolver for RkyvSnapshot {
    fn get_entity(&self, index: u32) -> Option<&IndexedEntity> {
        RkyvSnapshot::get_entity(self, index)
    }

    fn ancestors_of(&self, index: u32) -> Option<&[u32]> {
        let r = self.get_entity(index)?.ancestors;
        Some(&self.ancestors_arena[r.offset as usize..(r.offset + r.len) as usize])
    }

    fn resolve_attribute_path(&self, base: SortedSetRef, path: &[AttributeNameId]) -> Option<AttributeValueView<'_>> {
        if path.is_empty() {
            return None;
        }
        let pairs = &self.archived().attribute_pairs_arena[base.offset as usize..(base.offset + base.len) as usize];
        let mut current = pairs
            .binary_search_by_key(&path[0], |pair| deserialize_attr_name_id(&pair.0))
            .ok()
            .map(|i| &pairs[i].1)?;

        for &name in &path[1..] {
            match current.as_view() {
                AttributeValueView::Object(nested) => {
                    let nested_pairs = &self.archived().attribute_pairs_arena
                        [nested.offset as usize..(nested.offset + nested.len) as usize];
                    current = nested_pairs
                        .binary_search_by_key(&name, |pair| deserialize_attr_name_id(&pair.0))
                        .ok()
                        .map(|i| &nested_pairs[i].1)?;
                }
                _ => return None,
            }
        }

        Some(current.as_view())
    }

    fn attribute_set_values(&self, range: SortedSetRef) -> Vec<AttributeValueView<'_>> {
        self.archived().attribute_set_values_arena[range.offset as usize..(range.offset + range.len) as usize]
            .iter()
            .map(|v| v.as_view())
            .collect()
    }

    fn attribute_pairs_view(&self, range: SortedSetRef) -> Vec<(AttributeNameId, AttributeValueView<'_>)> {
        self.archived().attribute_pairs_arena[range.offset as usize..(range.offset + range.len) as usize]
            .iter()
            .map(|pair| (deserialize_attr_name_id(&pair.0), pair.1.as_view()))
            .collect()
    }
}

/// `ArchivedStringId`'s field is `pub(crate)` to `arbor-types`, so from
/// here it has to go through the (public) derived `Deserialize` impl
/// instead of direct field access -- cheap, a single `u32` copy, no
/// allocation.
fn deserialize_attr_name_id(archived: &arbor_types::ArchivedStringId<arbor_types::AttributeNameMarker>) -> AttributeNameId {
    rkyv::deserialize::<AttributeNameId, rkyv::rancor::Error>(archived).expect("deserialize AttributeNameId")
}

impl SnapshotOps for RkyvSnapshot {
    fn get_policy(&self, idx: u32) -> Option<&IndexedPolicy> {
        RkyvSnapshot::get_policy(self, idx)
    }
    fn get_policies_for_action(&self, action_idx: u32) -> ArborResult<&RoaringBitmap> {
        self.action_to_policies
            .get(&action_idx)
            .ok_or_else(|| ArborError::EntityNotFound(format!("Action not found {}", action_idx)))
    }
    fn effective_principal_of(&self, idx: u32) -> &[u32] {
        let r = self.get_entity(idx).and_then(|e| e.effective_principal_policies);
        Self::resolve(&self.effective_principal_arena, r)
    }
    fn effective_resource_of(&self, idx: u32) -> &[u32] {
        let r = self.get_entity(idx).and_then(|e| e.effective_resource_policies);
        Self::resolve(&self.effective_resource_arena, r)
    }
    fn split_policy_map_for_authorization(
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
    fn get_entities_of_type_for_policies(
        &self,
        policies: &[u32],
        entity_type_id: EntityTypeId,
        side: PolicySide,
    ) -> ArborResult<RoaringBitmap> {
        let et = self.indexed_entity_types.get(&entity_type_id)
            .ok_or_else(|| ArborError::EntityNotFound(format!("Entity type not found {:?}", entity_type_id)))?;
        let mut acc = RoaringBitmap::new();
        for &policy_idx in policies {
            let policy = SnapshotOps::get_policy(self, policy_idx)
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
    fn get_effective_policies_intersected(
        &self,
        entity_idx: u32,
        mask: &[u32],
        side: PolicySide,
    ) -> ArborResult<Vec<u32>> {
        self.get_entity(entity_idx)
            .ok_or_else(|| ArborError::EntityNotFound(format!("Entity not found {}", entity_idx)))?;
        let effective = match side {
            PolicySide::Principal => SnapshotOps::effective_principal_of(self, entity_idx),
            PolicySide::Resource => SnapshotOps::effective_resource_of(self, entity_idx),
        };
        Ok(Snapshot::intersect_sorted(effective, mask))
    }
    fn as_entity_resolver(&self) -> &dyn EntityResolver {
        self
    }
    fn uuid_to_index(&self, uuid: &Uuid) -> Option<u32> {
        self.uuid_to_index.get(uuid).copied()
    }
    fn index_to_uuid(&self, idx: u32) -> Option<Uuid> {
        self.index_to_uuid.get(idx as usize).copied().flatten()
    }
    fn get_entity_type_id_by_name(&self, name: &str) -> Option<EntityTypeId> {
        self.entity_type_name_to_id.get(name).copied()
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

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("rkyv error: {0}")]
    Rkyv(String),
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
// RkyvPackagedSnapshot
//
// Provenance metadata (version/created_at_ms/counts) plus an lz4-compressed
// rkyv archive of the `Snapshot` in `compressed_data`. This wrapper struct
// itself stays on bincode (cheap, tiny, no benefit to archiving a handful
// of scalar fields with rkyv) -- only the `Snapshot` payload uses rkyv.
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
pub struct RkyvPackagedSnapshot {
    pub version: u64,
    pub created_at_ms: i64,
    pub metadata: SnapshotMetadata,
    compressed_data: Vec<u8>,
}

impl RkyvPackagedSnapshot {
    /// Build an [`RkyvPackagedSnapshot`] from a raw [`Snapshot`].
    ///
    /// # Errors
    ///
    /// Returns [`SerializationError::Rkyv`] if archiving fails.
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

        let raw = rkyv::to_bytes::<rkyv::rancor::Error>(&snapshot).map_err(rkyv_err)?;
        let compressed_data = lz4_flex::compress_prepend_size(&raw);

        Ok(Self { version, created_at_ms, metadata, compressed_data })
    }

    /// Encode this packaged snapshot to bytes.
    pub fn serialize(&self) -> Result<Vec<u8>, SerializationError> {
        Ok(bincode::serialize(self)?)
    }

    /// Decode a packaged snapshot from bytes produced by [`RkyvPackagedSnapshot::serialize`].
    pub fn deserialize(bytes: &[u8]) -> Result<Self, SerializationError> {
        Ok(bincode::deserialize(bytes)?)
    }

    /// Unwraps the inner lz4-compressed rkyv archive bytes, for
    /// [`RkyvSnapshot::from_compressed_bytes`] to access directly. This
    /// can't hand back an owned `Snapshot` cheaply -- that would
    /// reintroduce the allocation-count problem `RkyvSnapshot` exists to
    /// avoid.
    pub fn into_compressed_data(self) -> Vec<u8> {
        self.compressed_data
    }
}
