use ipnet::IpNet;
use std::net::IpAddr;
use chrono::{DateTime, Utc};
use ordered_float::OrderedFloat;
use serde::{Deserialize, Serialize};
use rkyv::{Archive, Deserialize as RkyvDeserialize, Serialize as RkyvSerialize};
use crate::attributes::AttributeValue;
use crate::conditions::{VariableRef, VariableScope};
use crate::ids::EntityTypeId;
use crate::rkyv_with::{IpNetAsBits, OrderedFloatAsF64, TimestampMillis};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[derive(Archive, RkyvSerialize, RkyvDeserialize)]
pub enum ResolvedEntityIndex {
    Variable(VariableRef),
    Direct(u32),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[derive(Archive, RkyvSerialize, RkyvDeserialize)]
pub enum OpCode {
    // Stack manipulation
    PushInteger(i64),
    PushFloat(#[rkyv(with = OrderedFloatAsF64)] OrderedFloat<f64>),
    PushTimestamp(#[rkyv(with = TimestampMillis)] DateTime<Utc>),
    PushString(String),
    PushBool(bool),
    PushIpAddr(IpAddr),
    PushIpNetwork(#[rkyv(with = IpNetAsBits)] IpNet),
    PushEntityRef(u32),
    /// Push the value at the given attribute path onto the stack.
    /// Pushes StackValue::Missing if the path does not exist.
    /// Must always be consumed by a comparison or set opcode — never by And/Or/Not.
    PushVariable(VariableRef),
    /// Push a set literal onto the stack.
    PushSet(Vec<AttributeValue>),

    // Comparisons
    Eq,
    Neq,
    Lt,
    Lte,
    Gt,
    Gte,

    // Logical
    And,
    Or,
    Not,

    // Set operations
    In,
    Contains,
    ContainsAll,
    ContainsAny,

    // Attribute existence check — resolves the path and pushes Bool(exists).
    // Never produces Missing on the stack.
    HasAttribute(VariableRef),

    // String operations — Missing on either operand → false
    StartsWith,     // stack: [string, prefix] → bool
    EndsWith,       // stack: [string, suffix] → bool
    StringContains, // stack: [haystack, needle] → bool
    Like,           // stack: [string, pattern] → bool; * matches any char sequence

    // Entity type check — no stack operands; reads entity from context directly
    IsType(VariableScope, EntityTypeId),

    /// Hierarchy membership check against a root scope (principal or resource).
    ///
    /// No stack operands. The compiler resolves the target entity UUID to a
    /// snapshot index (`u32`) before emitting this opcode. The VM checks whether
    /// the entity at `scope` has `target_idx` in its `ancestors` roaring bitmap.
    ///
    /// Self-inclusive: the snapshot builder guarantees that each entity's own
    /// index is present in its `ancestors` bitmap, so `entity in entity` is true.
    InHierarchy(ResolvedEntityIndex, ResolvedEntityIndex),

    /// IP-in-network membership check.
    ///
    /// Stack: `[..., IpAddr, IpNetwork]` → `[..., bool]`
    ///
    /// Pops right (IpNetwork) then left (IpAddr) and checks whether the address
    /// is contained in the network. Missing on either operand → false.
    /// Wrong types → Invalid.
    InNetwork,

    // Control flow (not yet implemented)
    JumpIfFalse(u32),
    JumpIfTrue(u32),
    Jump(u32),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[derive(Archive, RkyvSerialize, RkyvDeserialize)]
pub struct CompiledCondition {
    pub instructions: Vec<OpCode>,
    pub dependencies: Vec<VariableRef>,
}
