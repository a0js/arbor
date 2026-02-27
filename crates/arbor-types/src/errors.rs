use thiserror::Error;

#[derive(Error, Debug)]
pub enum ArborError {
    #[error("Entity not found: {0}")]
    EntityNotFound(String),
    
    #[error("Conversion error: {0}")]
    ConversionError(String),

    #[error("Circular dependency cycle detected: {0}")]
    CircularDependency(String),

    #[error("Graph error: {0}")]
    Graph(#[from] GraphError),
}

#[derive(Error, Debug)]
pub enum GraphError {
    #[error("Node index not found: {0}")]
    NodeIndexNotFound(String),

    #[error("Node not found: {0}")]
    NodeNotFound(String),

    #[error("Type mismatch: expected {expected:?}, got {actual:?}")]
    TypeMismatch { expected: String, actual: String },
    
    #[error("Node already exists: {0}")]
    NodeAlreadyExists(String),
}

pub type ArborResult<T> = Result<T, ArborError>;