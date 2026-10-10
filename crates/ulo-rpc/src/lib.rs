//! The RPC transport (transports DESIGN §5): the [`Rpc`] marker and its context [`RpcCx`], one
//! frame grammar with JSON and CBOR codecs, pattern routing, all four call shapes, the
//! [`RpcClient`], and the [`link`] SPI each transport crate implements once.
//!
//! ```ignore
//! #[routes]
//! #[guards(rpc = ServiceTokenGuard)]
//! impl InvoicesController {
//!     #[ulo_rpc::message("invoices.create")]
//!     async fn create(&self, req: Payload<NewInvoice>, h: CallHeaders) -> Result<Invoice, BillingError> { /* .. */ }
//!
//!     #[ulo_rpc::event("user.created")]
//!     async fn on_user(&self, evt: Payload<UserCreated>) -> Result<(), BillingError> { /* .. */ }
//! }
//!
//! app.bind(ulo_rpc::Server::new(ulo_rpc_nats::Nats::url("nats://bus:4222")))
//! ```
//!
//! The call shape comes from the signature: an [`Inbound<T>`] parameter is a streamed request, a
//! stream return a streamed reply.

mod client;
mod client_module;
mod codec;
mod dispatch;
mod extract;
mod frame;
mod held;
mod server;
mod transport;

pub mod link;

#[doc(hidden)]
pub mod __private;

pub use client::{Call, Emit, RpcClient, RpcError, RpcStream, StreamCall, UnusableLink, ZeroTimeout};
pub use client_module::RpcClientModule;
pub use codec::Codec;
pub use extract::{Inbound, Payload};
pub use frame::{Data, ErrorBody, Frame, PayloadKind};
pub use link::{
    Ack, Capabilities, Delivery, DeliveryMode, FrameTooLarge, FrameUnencodable, Link, NoDestination, Ordering, Outbound,
    Pattern, ReplyPath, ReplyTo,
};
pub use server::Server;
pub use transport::{CallHeaders, LinkInfo, NoHandler, Reply, Rpc, RpcCx};
pub use ulo_rpc_macros::{event, message};
