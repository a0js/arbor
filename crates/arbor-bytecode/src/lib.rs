pub mod ast_evaluator;
mod bytecode_vm;

// Re-export commonly used types
pub use ast_evaluator::{
    AstEvaluator, EvaluationContext, EvaluationError, EvaluationNeed, EvaluationResult,
    GraphOracle,
};
