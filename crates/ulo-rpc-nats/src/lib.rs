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
//!
//! On the wire a request is the payload published on the pattern's subject with a reply subject,
//! an event the same without one, so `nats req invoices.create '{..}'` reaches a handler. Each
//! reply message carries one envelope frame, `res`, `err`, `item` or `end`. A streamed request
//! opens with an empty body and the header `ulo-t: open`; the server answers it on the reply
//! subject with `ulo-t: opened` before the caller sends anything more, and the request's items,
//! its end and every `cancel` travel on `ulo.rpc.control`, which every server instance reads, with
//! the call's reply subject as their reply subject.

mod link;

pub use link::Nats;
