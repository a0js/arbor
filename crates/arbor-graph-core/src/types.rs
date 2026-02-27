use arbor_types::{Entity, Policy, Action, ActionSet};

#[derive(Debug)]
pub enum NodeType {
    Entity(Box<Entity>),
    Policy(Box<Policy>),
    Action(Box<Action>),
    ActionSet(Box<ActionSet>),
    Placeholder,
}

impl NodeType {
    pub(crate) fn as_entity(&self) -> Option<&Entity> {
        if let NodeType::Entity(entity) = self {
            Some(entity.as_ref())
        } else {
            None
        }
    }

    pub(crate) fn as_policy(&self) -> Option<&Policy> {
        if let NodeType::Policy(policy) = self {
            Some(policy.as_ref())
        } else {
            None
        }
    }

    pub(crate) fn as_action(&self) -> Option<&Action> {
        if let NodeType::Action(action) = self {
            Some(action.as_ref())
        } else {
            None
        }
    }

    pub(crate) fn as_action_set(&self) -> Option<&ActionSet> {
        if let NodeType::ActionSet(action_set) = self {
            Some(action_set.as_ref())
        } else {
            None
        }
    }
}
