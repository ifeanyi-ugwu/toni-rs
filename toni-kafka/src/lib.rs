//! Apache Kafka transport for the Toni RPC gateway.
//!
//! Kafka is an event log, not a request-response bus, but it carries message
//! headers — so per-call metadata and the reply addressing ride headers (no
//! envelope), and request-response is emulated: a request names a reply topic
//! and a correlation id in its headers, the server publishes the reply there,
//! and the client routes replies back by correlation id.
//!
//! A pattern maps to a Kafka topic. Topics are expected to auto-create on the
//! broker (`auto.create.topics.enable`, the default); otherwise create the
//! request and reply topics out of band.
//!
//! - [`KafkaAdapter`] — server side; a `StreamConsumer` subscribed to the
//!   pattern topics, replying via a `FutureProducer`.
//! - [`KafkaClientTransport`] — client side; a producer plus one consumer on a
//!   private reply topic, correlation-routed.

mod kafka_adapter;
mod kafka_client_transport;
mod wire;

pub use kafka_adapter::KafkaAdapter;
pub use kafka_client_transport::KafkaClientTransport;
pub use toni::{RpcAdapter, RpcClient, RpcClientTransport};
