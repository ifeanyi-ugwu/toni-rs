use futures::stream::BoxStream;

use super::WsMessage;

/// The output of a WebSocket event handler.
///
/// Replaces `Result<Option<WsMessage>, WsError>` as the handler return type,
/// unifying all output modes under one type. The framework drives `Stream`
/// variants by spawning a task in the adapter layer; handlers never manage
/// that task directly.
pub enum WsHandlerOutput {
    /// No response to send.
    Empty,
    /// A single response message.
    Single(WsMessage),
    /// An unbounded sequence of messages pushed to the client until the
    /// stream exhausts or the connection closes.
    Stream(BoxStream<'static, WsMessage>),
}

impl std::fmt::Debug for WsHandlerOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WsHandlerOutput::Empty => write!(f, "WsHandlerOutput::Empty"),
            WsHandlerOutput::Single(msg) => write!(f, "WsHandlerOutput::Single({:?})", msg),
            WsHandlerOutput::Stream(_) => write!(f, "WsHandlerOutput::Stream(..)"),
        }
    }
}

impl From<WsMessage> for WsHandlerOutput {
    fn from(msg: WsMessage) -> Self {
        WsHandlerOutput::Single(msg)
    }
}

impl From<Option<WsMessage>> for WsHandlerOutput {
    fn from(opt: Option<WsMessage>) -> Self {
        match opt {
            Some(msg) => WsHandlerOutput::Single(msg),
            None => WsHandlerOutput::Empty,
        }
    }
}
