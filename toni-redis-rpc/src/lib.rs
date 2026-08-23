//! Redis Pub/Sub transport for the Toni RPC gateway.
//!
//! Redis Pub/Sub has no native request-reply: a publisher cannot address a
//! reply back to the caller the way NATS does with a reply-to inbox. This
//! transport emulates it with a correlation-keyed reply channel carried inside
//! a JSON request envelope. The envelope is also where per-call `metadata`
//! rides, since Redis Pub/Sub frames carry no headers of their own.
//!
//! - [`RedisAdapter`] — server side; subscribes one channel per registered
//!   pattern and publishes replies to the channel named in the request.
//! - [`RedisClientTransport`] — client side; runs a single background
//!   reply-router subscribed to all its in-flight reply channels at once.

mod redis_adapter;
mod redis_client_transport;
mod wire;

pub use redis_adapter::RedisAdapter;
pub use redis_client_transport::RedisClientTransport;
pub use toni::{RpcAdapter, RpcClient, RpcClientTransport};
