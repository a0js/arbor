use std::ops::Sub;
use rapidhash::RapidHashMap;
use roaring::{MultiOps, RoaringBitmap};
use uuid::Uuid;
use arbor_types::{
    EntityResolver, EntityTypeId, IndexedEntity, IndexedEntityType, IndexedNode, IndexedPolicy,
    IndexedPolicyTarget, ArborError, ArborResult,
};

pub struct Snapshot {
    pub uuid_to_index: RapidHashMap<Uuid, u32>,
    pub index_to_uuid: Vec<Option<Uuid>>,

    pub nodes: Vec<IndexedNode>,

    pub action_to_policies: RapidHashMap<u32, RoaringBitmap>,
    pub indexed_entity_types: RapidHashMap<EntityTypeId, IndexedEntityType>,

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

    pub fn get_policies_for_resource(&self, resource_idx: u32) -> ArborResult<RoaringBitmap> {
        let resource = self.get_entity(resource_idx)
            .ok_or_else(|| ArborError::EntityNotFound(format!("Entity not found {}", resource_idx)))?;
        let mut policies = vec![&self.all_resource_policies];

        if let Some(direct) = &resource.resource_of_policies {
            policies.push(direct);
        }

        let ancestral = resource.ancestors.iter()
            .filter_map(|anc_idx| {
                self.get_entity(anc_idx)
                    .and_then(|e| e.resource_of_policies.as_ref())
            })
            .collect::<Vec<_>>()
            .union() & &self.descendant_resource_policies;

        policies.push(&ancestral);

        if let Some(et) = self.indexed_entity_types.get(&resource.entity_type) {
            policies.push(&et.policies_targeting_resources_of_type);
        }

        Ok(policies.union())
    }

    pub fn get_policies_for_principal(&self, principal_idx: u32) -> ArborResult<RoaringBitmap> {
        let principal = self.get_entity(principal_idx)
            .ok_or_else(|| ArborError::EntityNotFound(format!("Entity not found {}", principal_idx)))?;
        let mut policies = vec![&self.all_principal_policies];

        if let Some(direct) = &principal.principal_of_policies {
            policies.push(direct);
        }

        let ancestral = principal.ancestors.iter()
            .filter_map(|anc_idx| {
                self.get_entity(anc_idx)
                    .and_then(|e| e.principal_of_policies.as_ref())
            })
            .collect::<Vec<_>>()
            .union() & &self.descendant_principal_policies;

        policies.push(&ancestral);

        if let Some(et) = self.indexed_entity_types.get(&principal.entity_type) {
            policies.push(&et.policies_targeting_principals_of_type);
        }

        Ok(policies.union())
    }

    pub fn get_policies_for_action(&self, action_idx: u32) -> ArborResult<RoaringBitmap> {
        self.action_to_policies
            .get(&action_idx)
            .cloned()
            .ok_or_else(|| ArborError::EntityNotFound(format!("Action not found {}", action_idx)))
    }

    pub fn get_principals_of_type_for_policy(
        &self,
        policy_idx: u32,
        entity_type_id: EntityTypeId,
    ) -> ArborResult<RoaringBitmap> {
        let policy = self.get_policy(policy_idx)
            .ok_or_else(|| ArborError::EntityNotFound(format!("Policy not found {}", policy_idx)))?;
        let et = self.indexed_entity_types.get(&entity_type_id)
            .ok_or_else(|| ArborError::EntityNotFound(format!("Entity type not found {:?}", entity_type_id)))?;
        match policy.principal_target {
            IndexedPolicyTarget::Entity(idx) => {
                Ok(if et.nodes_of_type.contains(idx) { RoaringBitmap::from([idx]) } else { RoaringBitmap::new() })
            }
            IndexedPolicyTarget::EntityWithDescendants(idx) => {
                let e = self.get_entity(idx)
                    .ok_or_else(|| ArborError::EntityNotFound(format!("Entity not found {}", idx)))?;
                Ok(&et.nodes_of_type & &e.descendants)
            }
            IndexedPolicyTarget::EntityType(tid) => {
                Ok(if tid == entity_type_id { et.nodes_of_type.clone() } else { RoaringBitmap::new() })
            }
            IndexedPolicyTarget::All => Ok(et.nodes_of_type.clone()),
        }
    }

    pub fn get_resources_of_type_for_policy(
        &self,
        policy_idx: u32,
        entity_type_id: EntityTypeId,
    ) -> ArborResult<RoaringBitmap> {
        let policy = self.get_policy(policy_idx)
            .ok_or_else(|| ArborError::EntityNotFound(format!("Policy not found {}", policy_idx)))?;
        let et = self.indexed_entity_types.get(&entity_type_id)
            .ok_or_else(|| ArborError::EntityNotFound(format!("Entity type not found {:?}", entity_type_id)))?;
        match policy.resource_target {
            IndexedPolicyTarget::Entity(idx) => {
                Ok(if et.nodes_of_type.contains(idx) { RoaringBitmap::from([idx]) } else { RoaringBitmap::new() })
            }
            IndexedPolicyTarget::EntityWithDescendants(idx) => {
                let e = self.get_entity(idx)
                    .ok_or_else(|| ArborError::EntityNotFound(format!("Entity not found {}", idx)))?;
                Ok(&et.nodes_of_type & &e.descendants)
            }
            IndexedPolicyTarget::EntityType(tid) => {
                Ok(if tid == entity_type_id { et.nodes_of_type.clone() } else { RoaringBitmap::new() })
            }
            IndexedPolicyTarget::All => Ok(et.nodes_of_type.clone()),
        }
    }

    pub fn get_actions_for_policy(&self, policy_idx: u32) -> ArborResult<RoaringBitmap> {
        self.get_policy(policy_idx)
            .map(|p| p.actions.clone())
            .ok_or_else(|| ArborError::EntityNotFound(format!("Policy not found {}", policy_idx)))
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
