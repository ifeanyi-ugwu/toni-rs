//! The NATS link for `ulo-rpc` (transports DESIGN §5.3): the payload as the message body with
//! NATS headers, replies on the `_INBOX` subject, stream items on the inbox. At-most-once;
//! ordered per publisher and subject; the maximum payload read from the server's `INFO`;
//! `Competing` through a queue group, the application's root module's full type path unless
//! `.group(..)` names one; no-responders maps to `Unavailable`; the drain is NATS's drain
//! protocol; `tls://` selects TLS.
//!
//! ```ignore
//! app.bind(ulo_rpc::Server::new(ulo_rpc_nats::Nats::url("nats://bus:4222")))
//! ```

mod link;

pub use link::{Nats};
