use uuid::Uuid;
use crate::attributes::ScalarValue;
use crate::conditions::{VariableRef, VariableScope};
use crate::ids::AttributeNameId;

pub enum OpCode {
    // Stack manipulation
    PushScalar(ScalarValue),
    PushEntityRef(Uuid),
    PushVariable { scope: VariableScope, path_len: u32 },

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

    // Attributes
    HasAttribute(AttributeNameId),

    // Control flow
    JumpIfFalse(u32),  // offset
    Jump(u32),
}

pub struct CompiledCondition {
    pub instructions: Vec<OpCode>,
    pub dependencies: Vec<VariableRef>,
}