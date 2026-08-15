//! What an RPC handler can ask for.
//!
//! Each of these is a [`FromContext<RpcContext>`], so a handler names what it
//! needs in any order and takes nothing it doesn't:
//!
//! ```rust,ignore
//! #[message_pattern("orders.place")]
//! async fn place(&self, Payload(order): Payload<PlaceOrder>) -> Result<Order, RpcError> {
//!     // no context parameter — this handler never reads one
//! }
//! ```
//!
//! A parameter of none of these types is the call's payload, deserialised into
//! it. That is the convention RPC handlers have always used, and it still holds:
//! `async fn place(&self, order: PlaceOrder, ctx: &mut RpcContext)` means what it
//! always did.

use std::convert::Infallible;
use std::fmt;

use serde::de::DeserializeOwned;

use crate::context::RpcContext;
use crate::extractors::{FromContext, Payload};
use crate::rpc::RpcData;

/// The call's payload, untouched.
impl FromContext<RpcContext> for RpcData {
    type Error = Infallible;

    async fn extract(ctx: &mut RpcContext) -> Result<Self, Self::Error> {
        Ok(ctx.data().clone())
    }
}

/// Why a call's data could not become the [`Payload`] a handler asked for.
#[derive(Debug)]
pub struct PayloadError(serde_json::Error);

impl fmt::Display for PayloadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for PayloadError {}

impl<T: DeserializeOwned> FromContext<RpcContext> for Payload<T> {
    type Error = PayloadError;

    async fn extract(ctx: &mut RpcContext) -> Result<Self, Self::Error> {
        ctx.data().parse().map(Payload).map_err(PayloadError)
    }
}
