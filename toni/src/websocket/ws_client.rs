use super::Session;
use std::collections::HashMap;

/// WebSocket client/connection
///
/// Represents a connected WebSocket client with its handshake data and the store scoped to the
/// connection. Cloned freely; every clone is the same connection and shares its session.
#[derive(Debug, Clone)]
pub struct WsClient {
    /// Client identifier (connection ID, session ID, etc.)
    pub id: String,

    /// Handshake information
    pub handshake: WsHandshake,

    /// State scoped to this connection, shared by every execution on it. Read it through
    /// [`session`](Self::session).
    ///
    /// Private because the handle must not be replaced: a `WsClient` is cloned freely, and swapping
    /// one clone's store detaches it from the connection while still looking connected.
    session: Session,
}

/// WebSocket handshake data
#[derive(Debug, Clone)]
pub struct WsHandshake {
    /// Query parameters from handshake URL
    pub query: HashMap<String, String>,

    /// Headers from handshake request
    pub headers: HashMap<String, String>,

    /// Remote address
    pub remote_addr: Option<String>,
}

impl WsClient {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            handshake: WsHandshake {
                query: HashMap::new(),
                headers: HashMap::new(),
                remote_addr: None,
            },
            session: Session::new(),
        }
    }

    pub fn with_handshake(mut self, handshake: WsHandshake) -> Self {
        self.handshake = handshake;
        self
    }

    /// State that outlives the executions on this connection — what a connect guard establishes and
    /// every later message reads.
    ///
    /// The bag for the execution being handled is a different thing with a shorter life: take
    /// [`Extensions`](crate::context::Extensions) as a handler parameter, or read
    /// [`WsContext::extensions`](crate::context::HandlerContext::extensions).
    pub fn session(&self) -> &Session {
        &self.session
    }
}
