//! What a WebSocket handler can ask for.
//!
//! Each of these is a [`FromContext<WsContext>`], so a handler names what it
//! needs in any order and takes nothing it doesn't:
//!
//! ```rust,ignore
//! #[subscribe_message("order.place")]
//! async fn place(&self, Payload(dto): Payload<PlaceOrder>, ext: Extensions) -> WsHandlerResult {
//!     // no `WsClient` here — this handler never uses it
//! }
//! ```
//!
//! Reaching for an HTTP extractor here is a trait-bound error: `Path<T>` carries
//! `FromContext<HttpContext>` and nothing else.

use std::convert::Infallible;
use std::fmt;

use serde::de::DeserializeOwned;

use crate::context::{Extensions, HandlerContext, WsContext};
use crate::extractors::FromContext;
use crate::websocket::{WsClient, WsMessage};

/// The client that sent the message.
///
/// A clone scoped to this message — see [`WsClient::extensions`], which is the
/// message's bag rather than the connection's.
impl FromContext<WsContext> for WsClient {
    type Error = Infallible;

    async fn extract(ctx: &mut WsContext) -> Result<Self, Self::Error> {
        Ok(ctx.client().clone())
    }
}

/// The raw frame, for a handler that wants to inspect it rather than have it
/// parsed.
impl FromContext<WsContext> for WsMessage {
    type Error = Infallible;

    async fn extract(ctx: &mut WsContext) -> Result<Self, Self::Error> {
        Ok(ctx.message().clone())
    }
}

/// The message's extension bag — the same one the gateway's enhancers wrote to.
impl FromContext<WsContext> for Extensions {
    type Error = Infallible;

    async fn extract(ctx: &mut WsContext) -> Result<Self, Self::Error> {
        Ok(ctx.extensions().clone())
    }
}

/// The message body, deserialised.
///
/// Text frames are parsed as JSON; binary frames are parsed as JSON over their
/// bytes. A frame that carries no payload — ping, pong, close — has nothing to
/// deserialise and fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Payload<T>(pub T);

/// Why a frame could not become the payload a handler asked for.
#[derive(Debug)]
pub enum PayloadError {
    /// The frame carries no payload to parse.
    NotData {
        /// The frame kind that arrived instead.
        frame: &'static str,
    },
    /// The frame carried a payload that did not match the expected shape.
    Parse(serde_json::Error),
}

impl fmt::Display for PayloadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotData { frame } => write!(
                f,
                "a `{frame}` frame carries no payload to deserialise; take `WsMessage` to handle \
                 these frames directly"
            ),
            Self::Parse(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for PayloadError {}

impl<T: DeserializeOwned> FromContext<WsContext> for Payload<T> {
    type Error = PayloadError;

    async fn extract(ctx: &mut WsContext) -> Result<Self, Self::Error> {
        let parsed = match ctx.message() {
            WsMessage::Text(text) => serde_json::from_str::<T>(text),
            WsMessage::Binary(bytes) => serde_json::from_slice::<T>(bytes),
            WsMessage::Ping(_) => return Err(PayloadError::NotData { frame: "Ping" }),
            WsMessage::Pong(_) => return Err(PayloadError::NotData { frame: "Pong" }),
            WsMessage::Close => return Err(PayloadError::NotData { frame: "Close" }),
        };
        parsed.map(Payload).map_err(PayloadError::Parse)
    }
}
