use futures::stream::BoxStream;

use super::{RpcData, RpcError};

/// The output of an RPC message handler.
///
/// Replaces `Option<RpcData>` as the `Ok` shape of
/// [`RpcHandlerResult`](super::RpcHandlerResult), adding the `Stream` variant
/// (ADR-0032). Adapters drive a `Stream` by draining it to the caller frame by
/// frame; handlers never manage that drain.
pub enum RpcHandlerOutput {
    /// No reply — the fire-and-forget answer of an `#[event_pattern]` handler.
    Empty,
    /// One reply.
    Single(RpcData),
    /// A sequence of replies, each framed to the caller, followed by an end
    /// marker. An `Err` item ends the stream: `AppError` renders as a final
    /// data frame carrying the canonical envelope, the framework variants as
    /// an error end that the caller's stream yields as its failure.
    Stream(BoxStream<'static, Result<RpcData, RpcError>>),
}

impl std::fmt::Debug for RpcHandlerOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RpcHandlerOutput::Empty => write!(f, "RpcHandlerOutput::Empty"),
            RpcHandlerOutput::Single(data) => write!(f, "RpcHandlerOutput::Single({:?})", data),
            RpcHandlerOutput::Stream(_) => write!(f, "RpcHandlerOutput::Stream(..)"),
        }
    }
}

impl From<RpcData> for RpcHandlerOutput {
    fn from(data: RpcData) -> Self {
        RpcHandlerOutput::Single(data)
    }
}

impl From<Option<RpcData>> for RpcHandlerOutput {
    fn from(opt: Option<RpcData>) -> Self {
        match opt {
            Some(data) => RpcHandlerOutput::Single(data),
            None => RpcHandlerOutput::Empty,
        }
    }
}
