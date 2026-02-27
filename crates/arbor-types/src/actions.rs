use uuid::Uuid;
use crate::ids::EntityTypeId;

const ACTION_NAMESPACE: Uuid = Uuid::from_u128(0x6ba7b810_9dad_11d1_80b4_00c04fd430c8);

#[derive(Debug)]
pub struct Action {
    pub id: Uuid,
    pub name: String,
    pub entity_type_id: EntityTypeId,
    pub description: Option<String>,
}

impl Action {
    pub fn get_action_name(&self) -> &str {
        &self.name
    }

    pub fn get_entity_type_id(&self) -> EntityTypeId {
        self.entity_type_id
    }

    pub fn hash_action_reference(action_ref: &str) -> Uuid {
        Uuid::new_v5(&ACTION_NAMESPACE, action_ref.as_bytes())
    }
}

#[derive(Debug)]
pub struct ActionSet {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub actions: Vec<Uuid>
}