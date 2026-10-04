//! The RabbitMQ link for `ulo-rpc`, AMQP 0-9-1 (transports DESIGN §5.3): a queue per pattern
//! through the default exchange, the payload as the body with AMQP headers, replies through
//! `reply_to` and `correlation_id`. At-least-once with the ack after the handler completes, so
//! handlers must be idempotent; ordered per queue with a single consumer; `Competing`; a
//! per-consumer prefetch from the server's `max_inflight`, 64 under `Default` or `Unlimited`; the
//! client channel in confirm mode with `mandatory` publishes, so `basic.return` maps to
//! `Unavailable`; an unhandled event is `basic.reject`ed without requeue; the drain cancels the
//! consumers (`basic.cancel`); `amqps://` selects TLS.
//!
//! ```ignore
//! app.bind(ulo_rpc::Server::new(ulo_rpc_rabbitmq::RabbitMq::url("amqp://guest:guest@mq:5672/%2f")))
//! ```

mod link;

pub use link::{RabbitMq};
