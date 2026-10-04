//! The Kafka link for `ulo-rpc` (transports DESIGN §5.3): a topic per pattern, the payload as the
//! body with Kafka headers, replies on a reply topic with a correlation header. Ordered per
//! partition, the partition key a caller-supplied ordering key or the client instance's id;
//! high latency for request-reply; the size limit from broker configuration; `Competing` through
//! the consumer group; the handler topics created at `bind`, so a stopped server's topic exists
//! and a miss is the client's `Timeout` under auto-create; the drain pauses and commits;
//! `security.protocol=SSL` selects TLS.
//!
//! ```ignore
//! app.bind(ulo_rpc::Server::new(ulo_rpc_kafka::Kafka::brokers("kafka:9092")))
//! ```

mod link;

pub use link::{Kafka};
