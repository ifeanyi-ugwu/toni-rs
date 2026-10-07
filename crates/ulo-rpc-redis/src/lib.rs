//! The Redis link for `ulo-rpc` (transports DESIGN §5.3): Pub/Sub on each pattern, the whole
//! frame as the message, replies on a per-client reply channel. At-most-once; ordered per
//! channel; `FanOut`, documented, the client dropping a second reply for an `id` it already
//! answered; a `PUBLISH` receiver count of zero maps to `Unavailable`, and so does a call waiting
//! when the client's Pub/Sub connection drops; `rediss://` selects TLS
//! under the crate's `tls` feature.
//!
//! ```ignore
//! app.bind(ulo_rpc::Server::new(ulo_rpc_redis::Redis::url("redis://cache:6379")))
//! ```
//!
//! On the wire every message is one whole frame in the link's codec. A `req` or `open` frame
//! carries the caller's reply channel in its headers as `ulo-reply`, which the server removes
//! before a handler reads them; `in`, `in_end` and `cancel` frames travel on `ulo:rpc:control`,
//! which every server subscribes to. A caller's frame ids are offset by a random base of its own,
//! so two callers never name the same call.

mod link;

pub use link::Redis;
