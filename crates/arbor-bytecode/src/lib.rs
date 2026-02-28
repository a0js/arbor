pub mod bytecode_vm;

// Re-export bytecode VM
pub use bytecode_vm::BytecodeVM;

// Evaluation types are in arbor-types::evaluation
// (ConditionResult, EvaluationContext, EvaluationError, EvaluationNeed)
