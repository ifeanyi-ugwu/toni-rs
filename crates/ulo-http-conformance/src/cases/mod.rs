//! The scenario list of transports DESIGN §3.8, one module per group, each scenario generic over
//! the [`Host`](crate::Host) under test and run in both modes.

pub mod disconnect;
pub mod drain;
pub mod errors;
pub mod extraction;
pub mod host_values;
pub mod lifecycle;
pub mod pre_dispatch;
pub mod routing;
pub mod routing_ext;
pub mod sse;
pub mod upgrade;
