//! The Redis broadcast adapter for `ulo-ws` (transports DESIGN §4.3): one Pub/Sub channel per room
//! or client, each process delivering to its own members. Membership stays local to each process
//! by construction, delivery is best effort and ordered per sender within one process, and the
//! API is the in-memory adapter's.
//!
//! ```ignore
//! #[module(imports = [ulo_ws::WsModule::for_root().broadcast(ulo_ws_redis::Redis::url("redis://cache:6379"))])]
//! pub struct AppModule;
//! ```
//!
//! The crate ships the adapter value and no module: a second module binding
//! `dyn BroadcastAdapter` beside `WsModule`'s would be `DuplicateBinding` at `wire()`.

use bytes::Bytes;
use futures_core::stream::BoxStream;
use ulo::{BoxError, BoxFuture};
use ulo_ws::{BroadcastAdapter, NodeId, Target};

/// The Redis adapter. The URL is parsed and the connection made lazily, at the first publish or
/// subscribe; `rediss://` selects TLS.
#[derive(Clone)]
pub struct Redis {
    pub(crate) url: String,
}

impl Redis {
    /// The adapter on `url`, `redis://host:6379` or `rediss://host:6380`.
    pub fn url(url: impl Into<String>) -> Self {
        Redis { url: url.into() }
    }
}

impl BroadcastAdapter for Redis {
    fn publish(&self, target: Target, frame: Bytes) -> BoxFuture<'static, Result<(), BoxError>> {
        let _ = (target, frame, &self.url);
        todo!()
    }

    fn subscribe(&self, node: NodeId) -> BoxStream<'static, (Target, Bytes)> {
        let _ = node;
        todo!()
    }
}
