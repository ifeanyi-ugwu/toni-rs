//! The Kafka link for `ulo-rpc` (transports DESIGN §5.3): a topic per pattern, the payload as the
//! body with Kafka headers, replies on a reply topic with a correlation header. Ordered per
//! partition, the partition key the client instance's id; high latency for request-reply; the
//! size limit the producer's `message.max.bytes`, 1,000,000 bytes, a broker configured lower
//! refusing with the same error; `Competing` through the consumer group, the application's root
//! module's full type path unless `.group(..)` names one; the handler topics created at `bind`, so
//! a stopped server's topic exists and a miss is the client's `Timeout` under auto-create; the
//! group's offsets committed at the topics' end at `bind` where it has none, so a request produced
//! while no instance consumes, or while a rebalance moves its partition, waits for the next owner
//! (`holds_unserved`); a reply published while the client is disconnected waits on its reply
//! topic for the client to reconnect (`durable_replies`); the drain pauses and commits; an
//! `ssl://` broker entry selects `security.protocol=SSL` under the `tls` feature.
//!
//! Kafka accepts that first commit only while the group has no member, so it anchors a topic only
//! when the group's first instance binds. A handler added in a deployment that rolls out while
//! the group runs gets a topic with no committed offset: each assignment of its partitions starts
//! at their end until a handler there settles a record and the next auto-commit records it. A
//! request produced to that topic before its first assignment, or while a rebalance moves its
//! partition, is skipped and its caller sees its own `Timeout`. Stopping every instance before
//! starting the new version anchors it.
//!
//! ```ignore
//! app.bind(ulo_rpc::Server::new(ulo_rpc_kafka::Kafka::brokers("kafka:9092")))
//! ```
//!
//! On the wire a request is the payload produced to the pattern's topic with the headers
//! `ulo-reply-to` and `ulo-correlation-id`, an event the same without them. Each record on the
//! reply topic carries one envelope frame, `res`, `err`, `item` or `end`, and the correlation id.
//! A streamed request opens with an empty body and the header `ulo-t: open`; the server answers it
//! with `ulo-t: opened` before the caller sends anything more, and the request's items, its end and
//! every `cancel` travel on the topic `ulo.rpc.control`, which every server instance reads in full,
//! carrying the call's correlation id. Kafka acknowledges no request: an offset is stored once its
//! handler completes and committed by the next auto-commit.

mod link;

pub use link::Kafka;
