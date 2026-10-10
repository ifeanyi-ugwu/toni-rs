//! The scenario list of transports DESIGN §5.2, one module per group, each scenario generic over
//! the [`Broker`](crate::Broker) under test.

pub(crate) mod app;

pub mod admission;
pub mod cancel;
pub mod client_close;
pub mod deadlines;
pub mod delivery;
pub mod drain;
pub mod errors;
pub mod events;
pub mod headers;
pub mod misses;
pub mod order;
pub mod payloads;
pub mod recovery;
pub mod streams;
pub mod threads;
pub mod unary;
