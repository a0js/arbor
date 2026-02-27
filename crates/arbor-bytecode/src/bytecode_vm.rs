//! Bytecode VM for condition evaluation
//!
//! TODO: Implement the bytecode VM (Task 1.5 in implementation plan)
//! This is currently a stub to allow the crate to compile.

use arbor_types::{Action, Attributes, IndexedEntityType, OpCode};

#[allow(dead_code)]
pub struct EvaluationContext {
    principal: IndexedEntityType,
    resource: IndexedEntityType,
    action: Action,
    context: Option<Attributes>,
}

pub struct BytecodeVM;

impl BytecodeVM {
    #[allow(unused_variables)]
    pub fn run(code: &[OpCode], context: &EvaluationContext) -> bool {
        // TODO: Implement bytecode VM execution
        // This will be implemented in Task 1.5 of the implementation plan
        todo!("Bytecode VM not yet implemented - use AST evaluator for now")
    }
}