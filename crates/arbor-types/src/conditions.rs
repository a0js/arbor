use crate::errors::ArborError;
use crate::errors::ArborError::ConversionError;
use crate::attributes::{AttributeValue, ScalarValue};
use crate::ids::AttributeNameId;
use uuid::Uuid;

/// Policy Condition Operand types
#[derive(Debug, Clone, PartialEq)]
pub enum Operand {
    // Literal values
    Scalar(ScalarValue),
    // References and variables
    EntityRef(Uuid),
    Set(Vec<Operand>),
    Variable(VariableRef),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Condition {
    Operand(Operand),

    And(Vec<Condition>),
    Or(Vec<Condition>),
    Not(Box<Condition>),

    Eq(Operand, Operand),
    Neq(Operand, Operand),

    Lt(Operand, Operand),
    Lte(Operand, Operand),
    Gt(Operand, Operand),
    Gte(Operand, Operand),

    In(Operand, Operand),          // e.g., value IN set
    Contains(Operand, Operand),    // e.g., set CONTAINS value
    ContainsAll(Operand, Operand), // e.g., set CONTAINS ALL values in another set
    ContainsAny(Operand, Operand), // e.g., set CONTAINS ANY value in another set

    HasAttribute(Operand, AttributeNameId), // e.g., entity HAS ATTRIBUTE "key"

    InNetwork(Operand, Operand), // e.g., ip() IN network
}

impl Condition {
    pub(crate) fn compute_dependencies(&self) -> Vec<VariableRef> {
        let mut deps = Vec::new();
        Self::find_condition_dependencies(self, &mut deps);
        deps.sort();
        deps.dedup();
        deps
    }

    fn find_condition_dependencies(condition: &Condition, deps: &mut Vec<VariableRef>) {
        match condition {
            Condition::Operand(op) => Self::find_operand_dependencies(op, deps),
            Condition::And(conds) | Condition::Or(conds) => {
                conds.iter().for_each(|c| Self::find_condition_dependencies(c, deps));
            }
            Condition::Not(cond) => Self::find_condition_dependencies(cond, deps),
            Condition::Eq(l, r) | Condition::Neq(l, r) | Condition::Lt(l, r)
            | Condition::Lte(l, r) | Condition::Gt(l, r) | Condition::Gte(l, r)
            | Condition::In(l, r) | Condition::Contains(l, r)
            | Condition::ContainsAll(l, r) | Condition::ContainsAny(l, r)
            | Condition::InNetwork(l, r) => {
                Self::find_operand_dependencies(l, deps);
                Self::find_operand_dependencies(r, deps);
            }
            Condition::HasAttribute(op, _) => Self::find_operand_dependencies(op, deps),
        }
    }

    fn find_operand_dependencies(operand: &Operand, deps: &mut Vec<VariableRef>) {
        match operand {
            Operand::Variable(var_ref) => deps.push(var_ref.clone()),
            Operand::Set(items) => items.iter().for_each(|i| Self::find_operand_dependencies(i, deps)),
            _ => {}
        }
    }
}

#[derive(Debug, Clone, PartialEq, Hash, PartialOrd, Eq, Ord)]
pub struct VariableRef {
    pub scope: VariableScope,
    pub path: Vec<AttributeNameId>,
}

#[derive(Debug, Clone, PartialEq, Hash, PartialOrd, Eq, Ord)]
pub enum VariableScope {
    Principal,
    Resource,
    Context,
}

impl TryFrom<AttributeValue> for Operand {
    type Error = ArborError;

    fn try_from(av: AttributeValue) -> Result<Self, Self::Error> {
        match av {
            AttributeValue::Scalar(sv) => Ok(Operand::Scalar(sv)),
            AttributeValue::EntityRef(eid) => Ok(Operand::EntityRef(eid)),
            AttributeValue::Set(vals) => {
                let mut operands = Vec::new();
                for val in vals {
                    operands.push(Operand::try_from(val)?);
                }
                Ok(Operand::Set(operands))
            }
            AttributeValue::Object(_) => Err(ConversionError(
                "Cannot convert nested object to operand".into(),
            )),
        }
    }
}

impl From<ScalarValue> for Operand {
    fn from(sv: ScalarValue) -> Self {
        Operand::Scalar(sv)
    }
}
