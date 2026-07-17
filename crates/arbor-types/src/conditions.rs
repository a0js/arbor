use std::net::IpAddr;
use chrono::{DateTime, Utc};
use ipnet::IpNet;
use ordered_float::OrderedFloat;
use serde::{Deserialize, Serialize};
use rkyv::{Archive, Deserialize as RkyvDeserialize, Serialize as RkyvSerialize};
use crate::errors::ArborError;
use crate::errors::ArborError::ConversionError;
use crate::attributes::AttributeValue;
use crate::ids::{AttributeNameId, EntityTypeId};

/// Policy Condition Operand types
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Operand {
    // Literal values
    String(String),
    Integer(i64),
    Float(OrderedFloat<f64>),
    Bool(bool),
    Timestamp(DateTime<Utc>),
    IpAddr(IpAddr),
    IpNetwork(IpNet),
    // References and variables
    EntityRef(u32),
    Set(Vec<Operand>),
    Variable(VariableRef),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

    // String operations
    StartsWith(Operand, Operand),     // string.starts_with(prefix)
    EndsWith(Operand, Operand),       // string.ends_with(suffix)
    StringContains(Operand, Operand), // string.contains(substring) — distinct from set Contains
    Like(Operand, Operand),           // glob pattern match: * matches any sequence of characters

    // Entity type check (e.g., `principal is Admin`)
    IsType(VariableScope, EntityTypeId),

    /// Hierarchy membership check (e.g., `principal in Group::"admins"`).
    ///
    /// Left operand is the entity variable (principal/resource); right operand
    /// is an EntityRef (UUID). The compiler resolves the UUID to a snapshot index
    /// before emitting the `InHierarchy` opcode — the VM never sees UUIDs.
    ///
    /// Semantics: true if the entity IS the target or is a descendant of it.
    /// Self-inclusive behaviour is guaranteed by the snapshot builder including
    /// each entity's own index in its `ancestors` bitmap.
    InHierarchy(Operand, Operand),

    InNetwork(Operand, Operand), // e.g., ip() IN network — V2
}

impl Condition {
    pub fn compute_dependencies(&self) -> Vec<VariableRef> {
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
            | Condition::StartsWith(l, r) | Condition::EndsWith(l, r)
            | Condition::StringContains(l, r) | Condition::Like(l, r)
            | Condition::InHierarchy(l, r) | Condition::InNetwork(l, r) => {
                Self::find_operand_dependencies(l, deps);
                Self::find_operand_dependencies(r, deps);
            }
            Condition::HasAttribute(op, _) => Self::find_operand_dependencies(op, deps),
            // IsType checks the entity's type directly — no attribute path dependency.
            Condition::IsType(_, _) => {}
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

#[derive(Debug, Clone, PartialEq, Hash, PartialOrd, Eq, Ord, Serialize, Deserialize)]
#[derive(Archive, RkyvSerialize, RkyvDeserialize)]
pub struct VariableRef {
    pub scope: VariableScope,
    pub path: Vec<AttributeNameId>,
}

#[derive(Debug, Clone, PartialEq, Hash, PartialOrd, Eq, Ord, Serialize, Deserialize)]
#[derive(Archive, RkyvSerialize, RkyvDeserialize)]
pub enum VariableScope {
    Principal,
    Resource,
    Context,
}

/// Simplified operand for ingestion; mirrors `Operand` but a variable is a
/// scope plus a human-readable dotted path (`Vec<String>`, resolved to
/// `AttributeNameId`s at graph-build time) instead of a pre-resolved
/// `VariableRef`, and an entity reference is a raw `Uuid` (resolved via
/// `uuid_to_index` at graph-build time) instead of a pre-resolved index --
/// same deferred-resolution shape as `PolicyTargetInput` vs `PolicyTarget`.
#[derive(Debug, Clone)]
pub enum OperandInput {
    String(String),
    Integer(i64),
    Float(OrderedFloat<f64>),
    Bool(bool),
    EntityRef(uuid::Uuid),
    Set(Vec<OperandInput>),
    Variable(VariableScope, Vec<String>),
}

/// Simplified condition for ingestion; mirrors `Condition` one-to-one except
/// `HasAttribute`, `IsType`, and `InNetwork` are omitted for now (no ingestion
/// path needs them yet -- the same variants can be added here later without
/// changing anything already built on this type).
#[derive(Debug, Clone)]
pub enum ConditionInput {
    Operand(OperandInput),
    And(Vec<ConditionInput>),
    Or(Vec<ConditionInput>),
    Not(Box<ConditionInput>),
    Eq(OperandInput, OperandInput),
    Neq(OperandInput, OperandInput),
    Lt(OperandInput, OperandInput),
    Lte(OperandInput, OperandInput),
    Gt(OperandInput, OperandInput),
    Gte(OperandInput, OperandInput),
    In(OperandInput, OperandInput),
    Contains(OperandInput, OperandInput),
    ContainsAll(OperandInput, OperandInput),
    ContainsAny(OperandInput, OperandInput),
    StartsWith(OperandInput, OperandInput),
    EndsWith(OperandInput, OperandInput),
    StringContains(OperandInput, OperandInput),
    Like(OperandInput, OperandInput),
    InHierarchy(OperandInput, OperandInput),
}

impl TryFrom<AttributeValue> for Operand {
    type Error = ArborError;

    fn try_from(av: AttributeValue) -> Result<Self, Self::Error> {
        match av {
            AttributeValue::String(s) => Ok(Operand::String(s)),
            AttributeValue::Integer(i) => Ok(Operand::Integer(i)),
            AttributeValue::Float(f) => Ok(Operand::Float(f)),
            AttributeValue::Bool(b) => Ok(Operand::Bool(b)),
            AttributeValue::Timestamp(t) => Ok(Operand::Timestamp(t)),
            AttributeValue::EntityRef(eid) => Ok(Operand::EntityRef(eid)),
            AttributeValue::IpAddr(_) => Err(ConversionError(
                "Cannot convert IpAddr to operand — use InNetwork condition instead".into(),
            )),
            AttributeValue::IpNetwork(net) => Ok(Operand::IpNetwork(net)),
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
