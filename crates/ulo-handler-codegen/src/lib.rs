//! A plain library of `syn` functions every transport's attribute macro calls, and `#[routes]`
//! with them: parameter analysis, the body-consumer check, the reply probe, the `use<>` rewrite on
//! streaming returns, the `__handler` protocol, and key and shared-value emission (transports
//! DESIGN §1). Not a proc-macro crate, so a user-written transport's macro crate depends on it as
//! the shipped ones do.
//!
//! A transport attribute's expansion, in order:
//! 1. [`protocol::take_handler_attr`] removes and parses the `__handler` tokens `#[routes]`
//!    appended; their absence means the attribute sits outside a `#[routes]` impl.
//! 2. [`params::analyze`] reads the receiver and the parameters.
//! 3. [`reply::rewrite_opaque_returns`] appends `+ use<>` to the return position's opaque types.
//! 4. The transport builds its handler value around [`emit::call_closure`], and
//!    [`emit::MountFn::emit`] writes the three items the protocol owes.

pub mod body;
pub mod emit;
pub mod keys;
pub mod params;
pub mod paths;
pub mod protocol;
pub mod reply;
pub mod shared;
pub mod util;

pub use paths::Paths;
