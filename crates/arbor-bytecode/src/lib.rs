pub mod bytecode_vm;
pub mod compiler;

// Re-export bytecode VM
pub use bytecode_vm::BytecodeVM;

// Re-export compiler
pub use compiler::{BytecodeCompiler, CompileError};

// Evaluation types are in arbor-types::evaluation
// (ConditionResult, EvaluationContext, EvaluationError, EvaluationNeed)
