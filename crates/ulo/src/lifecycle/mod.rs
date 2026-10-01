//! The lifecycle: the phase machine, the bounded runner every hook, construction and readiness
//! check goes through, the connect walk and the shutdown sequence (§9).

pub(crate) mod connect;
pub(crate) mod phase;
pub(crate) mod run;
pub(crate) mod shutdown;
