use thiserror::Error;

#[derive(Debug, Error, Clone)]
pub enum WsError {
    #[error("Connection closed: {0}")]
    ConnectionClosed(String),

    #[error("Invalid message format: {0}")]
    InvalidMessage(String),

    #[error("Authentication failed: {0}")]
    AuthFailed(String),

    #[error("Event not found: {0}")]
    EventNotFound(String),

    #[error("Internal error: {0}")]
    Internal(String),

    #[error("Broadcast error: {0}")]
    BroadcastError(String),
}

impl From<crate::websocket::BroadcastError> for WsError {
    fn from(err: crate::websocket::BroadcastError) -> Self {
        WsError::BroadcastError(err.to_string())
    }
}

impl crate::errors::AppError for WsError {
    fn kind(&self) -> crate::errors::ErrorKind {
        match self {
            Self::ConnectionClosed(_) => crate::errors::ErrorKind::Unavailable,
            Self::InvalidMessage(_) => crate::errors::ErrorKind::BadRequest,
            Self::AuthFailed(_) => crate::errors::ErrorKind::Unauthorized,
            Self::EventNotFound(_) => crate::errors::ErrorKind::NotFound,
            Self::Internal(_) => crate::errors::ErrorKind::Internal,
            Self::BroadcastError(_) => crate::errors::ErrorKind::Internal,
        }
    }
}

/// Reason for client disconnection
#[derive(Debug, Clone)]
pub enum DisconnectReason {
    ClientDisconnect,
    ServerShutdown,
    Timeout,
    Error(String),
}

impl DisconnectReason {
    pub fn error(msg: impl Into<String>) -> Self {
        Self::Error(msg.into())
    }
}
