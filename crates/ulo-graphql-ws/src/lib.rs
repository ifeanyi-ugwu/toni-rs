//! GraphQL subscriptions over graphql-transport-ws (transports DESIGN §7): one hand-written
//! `ulo_ws::Gateway` over `Engine::subscribe`, for every engine.
//!
//! The protocol's `connection_init`, `ping` and `pong` carry no `id` while expecting an answer, and
//! its `complete` from the client cancels an operation, so `#[ulo_ws::message]`'s envelope does
//! not fit. The gateway owns the subprotocol echo, `connection_init` once (a second closes 4429)
//! with its payload kept in the session, `connection_ack`, a `subscribe` before the ack closing
//! 4401, a duplicate `id` closing 4409, an unparseable message closing 4400, an init missing
//! `connection_init_timeout` closing 4408, `ping` answered with `pong`, one execution per
//! `subscribe` writing `next` items and `complete` or `error`, and `complete` from the client
//! cancelling that execution with `ClientCancelled`.

mod gateway;

pub use gateway::{ConnectionInit, GraphqlWs};
