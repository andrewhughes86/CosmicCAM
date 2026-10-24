use thiserror::Error;

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum EngineError {
    #[error("unknown command: {0}")]
    UnknownCommand(String),
    #[error("invalid parameters: {0}")]
    InvalidParams(String),
    #[error("not supported yet: {0}")]
    NotSupported(String),
    #[error("command is not available right now: {0}")]
    Disabled(String),
    #[error("internal error: {0}")]
    Internal(String),
}

pub type EngineResult<T> = Result<T, EngineError>;

impl EngineError {
    /// Stable machine-readable kind for the control protocol and the Python API.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::UnknownCommand(_) => "unknown_command",
            Self::InvalidParams(_) => "invalid_params",
            Self::NotSupported(_) => "not_supported",
            Self::Disabled(_) => "disabled",
            Self::Internal(_) => "internal",
        }
    }
}

/// `{"kind": ..., "message": ...}`
pub fn error_json(e: &EngineError) -> serde_json::Value {
    serde_json::json!({ "kind": e.kind(), "message": e.to_string() })
}
