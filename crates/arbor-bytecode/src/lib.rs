pub mod bytecode_vm;
pub mod compiler;

// Re-export bytecode VM
pub use bytecode_vm::BytecodeVM;

// Re-export compiler
pub use compiler::BytecodeCompiler;

#[cfg(any(test, feature = "test_utils"))]
pub mod ast_evaluator;

#[cfg(any(test, feature = "test_utils"))]
pub use crate::ast_evaluator::evaluate_ast;

// Evaluation types are in arbor-types::evaluation
// (ConditionResult, EvaluationContext, EvaluationError, EvaluationNeed)
