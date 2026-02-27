use std::ops::Sub;
use rapidhash::RapidHashMap;
use roaring::{MultiOps, RoaringBitmap};
use uuid::Uuid;
use arbor_types::{EntityTypeId, IndexedPolicyTarget, IndexedEntity, IndexedPolicy, IndexedEntityType, ArborError, ArborResult};

pub struct Snapshot {
    pub uuid_to_index: RapidHashMap<Uuid, u32>,
    pub index_to_uuid: Vec<Option<Uuid>>,

    pub indexed_entities: RapidHashMap<u32, IndexedEntity>,
    pub indexed_policies: RapidHashMap<u32, IndexedPolicy>,
    pub action_to_policies: RapidHashMap<u32, RoaringBitmap>,
    pub indexed_entity_types: RapidHashMap<EntityTypeId, IndexedEntityType>,

    pub all_principal_policies: RoaringBitmap,
    pub all_resource_policies: RoaringBitmap,
    pub conditional_policies: RoaringBitmap,
    pub forbidding_policies: RoaringBitmap,
    pub descendant_principal_policies: RoaringBitmap,
    pub descendant_resource_policies: RoaringBitmap,
}

impl Snapshot {
    pub fn new() -> Self {
        Self {
            uuid_to_index: RapidHashMap::default(),
            index_to_uuid: Vec::new(),
            indexed_entities: RapidHashMap::default(),
            indexed_entity_types: RapidHashMap::default(),
            all_principal_policies: RoaringBitmap::new(),
            all_resource_policies: RoaringBitmap::new(),
            conditional_policies: RoaringBitmap::new(),
            forbidding_policies: RoaringBitmap::new(),
            descendant_principal_policies: RoaringBitmap::new(),
            descendant_resource_policies: RoaringBitmap::new(),
            indexed_policies: RapidHashMap::default(),
            action_to_policies: RapidHashMap::default(),
        }
    }
    pub fn get_policies_for_resource(&self, resource_bit: u32) -> ArborResult<RoaringBitmap> {
        let resource = self.indexed_entities.get(&resource_bit).ok_or_else(|| ArborError::EntityNotFound(format!("Entity not found {}", resource_bit)))?;
        let mut policies = vec![&self.all_resource_policies];

        if let Some(direct_resource_policies) = &resource.resource_of_policies {
            policies.push(direct_resource_policies)
        }

        let ancestral_related_policies = resource.ancestors.iter()
            .filter_map(|ancestor_idx| {
                self.indexed_entities.get(&ancestor_idx)
                    .and_then(|ancestor| ancestor.resource_of_policies.as_ref())
            })
            .collect::<Vec<_>>()
            .union() & &self.descendant_resource_policies;

        policies.push(&ancestral_related_policies);

        if let Some(entity_type) = self.indexed_entity_types.get(&resource.entity_type) {
            policies.push(&entity_type.policies_targeting_resources_of_type)
        };

        Ok(policies.union())
    }

    pub fn get_policies_for_principal(&self, principal_bit: u32) -> ArborResult<RoaringBitmap> {
        let principal = self.indexed_entities.get(&principal_bit).ok_or_else(|| ArborError::EntityNotFound(format!("Entity not found {}", principal_bit)))?;
        let mut policies = vec![&self.all_principal_policies];

        if let Some(direct_principal_policies) = &principal.principal_of_policies {
            policies.push(direct_principal_policies)
        }

        let ancestral_related_policies = principal.ancestors.iter()
            .filter_map(|ancestor_idx| {
                self.indexed_entities.get(&ancestor_idx)
                    .and_then(|ancestor| ancestor.principal_of_policies.as_ref())
            })
            .collect::<Vec<_>>()
            .union() & &self.descendant_principal_policies;

        policies.push(&ancestral_related_policies);

        if let Some(entity_type) = self.indexed_entity_types.get(&principal.entity_type) {
            policies.push(&entity_type.policies_targeting_principals_of_type)
        };

        Ok(policies.union())
    }

    pub fn get_policies_for_action(&self, action_bit: u32) -> ArborResult<RoaringBitmap> {
        let policies = self.action_to_policies.get(&action_bit).ok_or_else(|| ArborError::EntityNotFound(format!("Entity not found {}", action_bit)))?;
        Ok(policies.clone())
    }

    pub fn get_principals_of_type_for_policy(&self, policy_idx: u32, entity_type_id: EntityTypeId) -> ArborResult<RoaringBitmap> {
        let policy = self.indexed_policies.get(&policy_idx).ok_or_else(|| ArborError::EntityNotFound(format!("Entity not found {}", policy_idx)))?;
        let entity_type = self.indexed_entity_types.get(&entity_type_id).ok_or_else(|| ArborError::EntityNotFound(format!("Entity Type not found {:?}", entity_type_id)))?;
        match policy.principal_target {
            IndexedPolicyTarget::Entity(entity_idx) => {
                if entity_type.nodes_of_type.contains(entity_idx) {
                    Ok(RoaringBitmap::from([entity_idx]))
                } else {
                    Ok(RoaringBitmap::default())
                }
            },
            IndexedPolicyTarget::EntityWithDescendants(entity_idx) => {
                let entity = self.indexed_entities.get(&entity_idx).ok_or_else(|| ArborError::EntityNotFound(format!("Entity not found {}", entity_idx)))?;
                Ok(&entity_type.nodes_of_type & &entity.descendants)
            },
            IndexedPolicyTarget::EntityType(policy_entity_type_id) => {
                if policy_entity_type_id == entity_type_id {
                    Ok(entity_type.nodes_of_type.clone())
                } else {
                    Ok(RoaringBitmap::default())
                }
            },
            IndexedPolicyTarget::All => {
                Ok(entity_type.nodes_of_type.clone())
            }

        }
    }

    pub fn get_resources_of_type_for_policy(&self, policy_idx: u32, entity_type_id: EntityTypeId) -> ArborResult<RoaringBitmap> {
        let policy = self.indexed_policies.get(&policy_idx).ok_or_else(|| ArborError::EntityNotFound(format!("Entity not found {}", policy_idx)))?;
        let entity_type = self.indexed_entity_types.get(&entity_type_id).ok_or_else(|| ArborError::EntityNotFound(format!("Entity Type not found {:?}", entity_type_id)))?;
        match policy.resource_target {
            IndexedPolicyTarget::Entity(entity_idx) => {
                if entity_type.nodes_of_type.contains(entity_idx) {
                    Ok(RoaringBitmap::from([entity_idx]))
                } else {
                    Ok(RoaringBitmap::default())
                }
            },
            IndexedPolicyTarget::EntityWithDescendants(entity_idx) => {
                let entity = self.indexed_entities.get(&entity_idx).ok_or_else(|| ArborError::EntityNotFound(format!("Entity not found {}", entity_idx)))?;
                Ok(&entity_type.nodes_of_type & &entity.descendants)
            },
            IndexedPolicyTarget::EntityType(policy_entity_type_id) => {
                if policy_entity_type_id == entity_type_id {
                    Ok(entity_type.nodes_of_type.clone())
                } else {
                    Ok(RoaringBitmap::default())
                }
            },
            IndexedPolicyTarget::All => {
                Ok(entity_type.nodes_of_type.clone())
            }

        }
    }

    pub fn get_actions_for_policy(&self, policy_idx: u32) -> ArborResult<RoaringBitmap> {
        let policy = self.indexed_policies.get(&policy_idx).ok_or_else(|| ArborError::EntityNotFound(format!("Entity not found {}", policy_idx)))?;
        Ok(policy.actions.clone())
    }


    pub fn split_policy_map_for_authorization(
        &self,
        policy_bitmap: &RoaringBitmap,
    ) -> (RoaringBitmap, RoaringBitmap, RoaringBitmap, RoaringBitmap) {
        let conditional_policies = policy_bitmap & &self.conditional_policies;
        let unconditional_policies = policy_bitmap.sub(&self.conditional_policies);

        let unconditional_forbidding = &unconditional_policies & &self.forbidding_policies;
        let conditional_forbidding = &conditional_policies & &self.forbidding_policies;
        let unconditional_permitting = unconditional_policies.sub(&self.forbidding_policies);
        let conditional_permitting = conditional_policies.sub(&self.forbidding_policies);

        (
            unconditional_forbidding,
            conditional_forbidding,
            unconditional_permitting,
            conditional_permitting,
        )
    }
}
