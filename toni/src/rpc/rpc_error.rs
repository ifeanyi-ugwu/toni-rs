use thiserror::Error;

#[derive(Error, Debug, Clone)]
pub enum RpcError {
    #[error("Pattern not found: {0}")]
    PatternNotFound(String),
    #[error("Guard rejected message: {0}")]
    Forbidden(String),
    #[error("Internal error: {0}")]
    Internal(String),
}

impl crate::errors::AppError for RpcError {
    fn kind(&self) -> crate::errors::ErrorKind {
        match self {
            Self::PatternNotFound(_) => crate::errors::ErrorKind::NotFound,
            Self::Forbidden(_) => crate::errors::ErrorKind::Forbidden,
            Self::Internal(_) => crate::errors::ErrorKind::Internal,
        }
    }
}
