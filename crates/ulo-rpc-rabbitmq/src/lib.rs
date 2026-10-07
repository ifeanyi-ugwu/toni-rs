//! The RabbitMQ link for `ulo-rpc`, AMQP 0-9-1 (transports DESIGN §5.3): a queue per pattern
//! through the default exchange, the payload as the body with AMQP headers, replies through
//! `reply_to` and `correlation_id`. At-least-once with the ack after the handler completes, so
//! handlers must be idempotent; ordered per queue with a single consumer; `Competing`; a
//! per-consumer prefetch following the server's `max_inflight`, 64 when it sets no bound; the
//! client channel in confirm mode with `mandatory` publishes, so `basic.return` maps to
//! `Unavailable`; an unhandled event is `basic.reject`ed without requeue; a call waiting when the
//! client's connection drops is `Unavailable`, since a direct reply-to address dies with its
//! channel; the drain cancels the consumers (`basic.cancel`); `amqps://` selects TLS.
//!
//! ```ignore
//! app.bind(ulo_rpc::Server::new(ulo_rpc_rabbitmq::RabbitMq::url("amqp://guest:guest@mq:5672/%2f")))
//! ```
//!
//! On the wire a request is the payload published to the pattern's queue with `reply_to` and
//! `correlation_id`, an event the same without them. The client receives replies through
//! RabbitMQ's direct reply-to, each message one envelope frame, `res`, `err`, `item` or `end`. A
//! streamed request opens with an empty body and the header `ulo-t: open`; the server answers it
//! with `ulo-t: opened` before the caller sends anything more, and the request's items, its end and
//! every `cancel` travel through the fanout exchange `ulo.rpc.control`, which binds one queue per
//! server instance, carrying the call's `correlation_id`. A queue outlives the server that
//! declared it, so a request sent while no instance consumes waits in the queue, and a caller sees
//! its own `Timeout` rather than `Unavailable`: the link declares `holds_unserved`.

mod link;

pub use link::RabbitMq;
