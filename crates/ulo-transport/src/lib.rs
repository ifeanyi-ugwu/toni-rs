//! The transport-neutral half of every `ulo` transport (transports DESIGN §1, §2): how a handler
//! parameter is built from a call ([`FromCall`]), how a handler's answer becomes a reply
//! ([`IntoReply`]), the one error model ([`CallError`], [`ErrorKind`], [`Classify`], [`Details`]),
//! extraction failures ([`ExtractError`]), declarative validation ([`Valid`], [`Validate`]),
//! declared metadata ([`Metadata`]), load shedding ([`Admission`]), the count a server setting
//! bounds ([`Count`]), call spans ([`span`]), stream-end tracking ([`Tracked`]), the tasks a
//! server spawns ([`TaskSet`]) and the one way a server builds its `prepare` failure
//! ([`prepare`]).
//!
//! Each transport says only how these look on its wire. A transport's `Cx` implements
//! `AsRef<ExecutionRef>`, which is how the impls here reach the call's execution.

mod admission;
mod count;
mod details;
mod error;
mod extract;
mod reply;
mod task_set;
mod tracked;
mod validate;

pub mod prepare;
pub mod span;

#[doc(hidden)]
pub mod __private;

pub use admission::{Admission, ConnectionAdmission, Permit};
pub use count::Count;
pub use details::{Detail, Details, FieldViolation, Link};
pub use error::{CallError, Classify, ErrorKind};
pub use extract::{ExtractError, FromCall, Injected};
pub use reply::{IntoReply, IntoReplyError};
pub use task_set::TaskSet;
pub use tracked::Tracked;
pub use ulo::{MetaTier, Metadata};
pub use ulo_transport_macros::{Classify, Validate};
pub use validate::{Valid, Validate};

#[cfg(feature = "validator")]
pub use validate::validator_bridge;
