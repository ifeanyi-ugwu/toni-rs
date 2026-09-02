use serde::{Deserialize, Serialize};

/// WebSocket message data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WsMessage {
    /// Text message (JSON, plain text, etc.)
    Text(String),

    /// Binary message
    Binary(Vec<u8>),

    /// Ping frame
    Ping(Vec<u8>),

    /// Pong frame
    Pong(Vec<u8>),

    /// Close frame, carrying the code and reason when the sender gave one.
    Close(Option<CloseFrame>),
}

/// Why a connection is closing, as RFC 6455 frames it: a status code and a
/// short reason the peer can read.
///
/// A browser surfaces both on its `close` event, which makes this the only
/// channel a server has for explaining a refusal — the handshake is over by
/// the time application code runs, so there is no HTTP status left to send.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloseFrame {
    /// RFC 6455 status code — 1008 for a policy refusal, 1011 for a server
    /// fault, 1013 to ask the caller to come back later.
    pub code: u16,
    /// Human-readable explanation, at most 123 bytes on the wire.
    pub reason: String,
}

impl WsMessage {
    pub fn text(data: impl Into<String>) -> Self {
        Self::Text(data.into())
    }

    pub fn binary(data: Vec<u8>) -> Self {
        Self::Binary(data)
    }

    pub fn close() -> Self {
        Self::Close(None)
    }

    /// A close frame carrying a code and a reason.
    ///
    /// The whole close payload is capped at 125 bytes by RFC 6455, two of
    /// which are the code, so the reason is truncated to 123 bytes on a
    /// character boundary rather than producing a frame the peer rejects.
    pub fn close_with(code: u16, reason: impl Into<String>) -> Self {
        const MAX_REASON: usize = 123;
        let mut reason = reason.into();
        if reason.len() > MAX_REASON {
            let end = (0..=MAX_REASON)
                .rev()
                .find(|i| reason.is_char_boundary(*i))
                .unwrap_or(0);
            reason.truncate(end);
        }
        Self::Close(Some(CloseFrame { code, reason }))
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            WsMessage::Text(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_binary(&self) -> Option<&[u8]> {
        match self {
            WsMessage::Binary(b) => Some(b),
            _ => None,
        }
    }
}
