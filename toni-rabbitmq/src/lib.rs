//! RabbitMQ (AMQP 0-9-1) transport for the Toni RPC gateway.
//!
//! Unlike Redis Pub/Sub, AMQP carries request-response natively: a message
//! has a `reply_to` and `correlation_id`, and per-call metadata rides in the
//! AMQP headers table. So there is no envelope to invent — the payload is raw
//! `RpcData` bytes (as in NATS) and the addressing lives in the message
//! properties.
//!
//! - [`RabbitMqAdapter`] — server side; declares one queue per registered
//!   pattern (routed by the default exchange), consumes, and publishes replies
//!   to the delivery's `reply_to` with the matching `correlation_id`.
//! - [`RabbitMqClientTransport`] — client side; uses RabbitMQ direct reply-to
//!   (`amq.rabbitmq.reply-to`), so request-response needs no real reply queue.

mod rabbitmq_adapter;
mod rabbitmq_client_transport;
mod wire;

pub use rabbitmq_adapter::RabbitMqAdapter;
pub use rabbitmq_client_transport::RabbitMqClientTransport;
pub use toni::{RpcAdapter, RpcClient, RpcClientTransport};
