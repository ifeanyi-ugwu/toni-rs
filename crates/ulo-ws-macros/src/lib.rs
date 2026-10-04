//! The WebSocket attributes, re-exported by `ulo-ws`: `#[gateway(..)]` on a `#[routes]` impl and
//! `#[message("event")]` on its handler methods, each a thin layer over `ulo-handler-codegen`.
//!
//! ```ignore
//! #[routes]
//! #[ulo_ws::gateway(path = "/chat", namespace = "lobby", event = "event", session = ChatSession,
//!                   connect_guards(TokenGuard), subprotocols = ["chat.v1"])]
//! impl ChatGateway {
//!     #[ulo_ws::message("chat.send")]
//!     async fn send(&self, msg: Payload<ChatMessage>) -> Result<Ack, ChatError> { /* .. */ }
//! }
//! ```

mod gateway;
mod message;

use proc_macro::TokenStream;

/// The gateway's configuration on a `#[routes]` impl, which `#[routes]` passes through and this
/// attribute turns into `impl ulo_ws::GatewayConfig`: `path`, `namespace`, `event`, `codec`,
/// `subprotocols`, `session` or `session_with`, `connect_guards(..)`, `refuse`, `overflow` and the
/// limits. A gateway with no `#[message]` handler is refused at compile time.
#[proc_macro_attribute]
pub fn gateway(attr: TokenStream, item: TokenStream) -> TokenStream {
    gateway::expand(attr.into(), item.into()).unwrap_or_else(syn::Error::into_compile_error).into()
}

/// A message handler for `event`, one execution per message. `#[message("cancel")]` is refused:
/// `cancel` is the envelope's reserved event.
#[proc_macro_attribute]
pub fn message(attr: TokenStream, item: TokenStream) -> TokenStream {
    message::expand(attr.into(), item.into()).unwrap_or_else(syn::Error::into_compile_error).into()
}
