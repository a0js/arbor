use uuid::Uuid;
use crate::attributes::{AttributeValue, ScalarValue};
use crate::conditions::{VariableRef, VariableScope};
use crate::ids::EntityTypeId;

#[derive(Debug, Clone, PartialEq)]
pub enum OpCode {
    // Stack manipulation
    PushScalar(ScalarValue),
    PushEntityRef(Uuid),
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
    InHierarchy(VariableScope, u32),

    /// Hierarchy membership check where the entity to test is stored as an
    /// attribute on the principal or resource (e.g., `principal.manager in AdminGroup`).
    ///
    /// No stack operands. The VM resolves `var_ref` to an `AttributeValue::EntityRef`,
    /// uses the `EntityResolver` in `EvaluationContext` to look up that entity in the
    /// snapshot, then checks its `ancestors` bitmap for `target_idx`.
    ///
    /// Missing attribute or unresolvable UUID → `false`.
    /// Wrong attribute type → `Invalid`.
    /// No `EntityResolver` in context → `Invalid`.
    InHierarchyVar(VariableRef, u32),

    /// Set membership with hierarchy expansion.
    ///
    /// Stack: `[..., set]` → `[..., bool]`
    ///
    /// Pops a set of `EntityRef`s from the stack and checks whether ANY element
    /// is the target or a descendant of it. `target_idx` is the pre-resolved
    /// snapshot index of the target ancestor (compiler resolves UUID → u32).
    ///
    /// Missing set → `false`. Non-EntityRef element in set → `Invalid`.
    /// Unresolvable UUID in set → skipped (treated as not in hierarchy).
    /// No `EntityResolver` in context → `Invalid`.
    ContainsInHierarchy(u32),

    // Control flow (not yet implemented)
    JumpIfFalse(u32),
    Jump(u32),
}

#[derive(Debug, Clone)]
pub struct CompiledCondition {
    pub instructions: Vec<OpCode>,
    pub dependencies: Vec<VariableRef>,
}
