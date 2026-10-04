//! The Redis link for `ulo-rpc` (transports DESIGN §5.3): Pub/Sub on each pattern, the whole
//! frame as the message, replies on a per-client reply channel. At-most-once; ordered per
//! channel; `FanOut`, documented, the client dropping a second reply for an `id` it already
//! answered; a `PUBLISH` receiver count of zero maps to `Unavailable`; `rediss://` selects TLS.
//!
//! ```ignore
//! app.bind(ulo_rpc::Server::new(ulo_rpc_redis::Redis::url("redis://cache:6379")))
//! ```

mod link;

pub use link::{Redis};
