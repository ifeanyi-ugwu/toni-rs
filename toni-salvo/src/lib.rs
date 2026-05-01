//! # toni-salvo
//!
//! [Salvo](https://salvo.rs) adapter for the toni framework. Implements both
//! `HttpAdapter` and `WebSocketAdapter`, so a single adapter type covers HTTP
//! routes, same-port WebSocket upgrades, and separate-port WebSocket servers.
//!
//! ## Usage
//!
//! ```ignore
//! use toni_salvo::SalvoAdapter;
//!
//! #[tokio::main]
//! async fn main() {
//!     let mut app = ToniFactory::new()
//!         .create_with(AppModule::module_definition())
//!         .await;
//!     app.use_http_adapter(SalvoAdapter::new(), 3000, "127.0.0.1").unwrap();
//!     // Only needed for `#[websocket_gateway(port = N)]` gateways.
//!     app.use_websocket_adapter(SalvoAdapter::new()).unwrap();
//!     app.start().await.unwrap();
//! }
//! ```
//!
//! ## Routing
//!
//! Toni uses `:param` path syntax; salvo uses `{param}`. The adapter rewrites
//! routes at bind time, so handlers stay written in toni's style. A catch-all
//! fallback is registered at `{**rest}` to produce the standard toni 404
//! response for unmatched paths, with the global middleware chain applied.
//!
//! ## HTTP methods
//!
//! GET, POST, PUT, DELETE, PATCH, HEAD, and OPTIONS dispatch through salvo's
//! corresponding `Router` filters. TRACE and CONNECT are attached via
//! `Router::goal()` because salvo's `Router` does not expose `.trace()` or
//! `.connect()` — the handler runs on a path match regardless of method, so
//! routes intended for TRACE/CONNECT should not share a path with handlers
//! that expect exact-method dispatch.
//!
//! ## Request bodies
//!
//! Request bodies are forwarded as a stream rather than buffered. Toni's
//! `BodyStream` extractor receives frames frame-by-frame; `Bytes`, `Json`,
//! and friends call `RequestBody::collect` which drains the stream on demand.
//! Salvo's default 64 KiB body cache (used by `Request::payload`) is bypassed
//! — there is no built-in size limit on what reaches the handler.
//!
//! ## Response bodies
//!
//! Toni's [`Body::stream`](toni::http_helpers::Body::stream) is forwarded to
//! salvo as `ResBody::Boxed`, so chunks reach the client incrementally. SSE
//! and other long-lived streaming responses work without buffering.
//!
//! ## WebSockets
//!
//! - **Same-port** upgrades go through salvo's `WebSocketUpgrade` on the HTTP
//!   listener. Sub-protocols (`Sec-WebSocket-Protocol`) are negotiated by
//!   echoing the first protocol the client offers.
//! - **Separate-port** gateways (`#[websocket_gateway(port = N)]`) bind their
//!   own salvo server on the requested port. `port = 0` lets the OS assign a
//!   distinct listener.
//! - Outbound messages are buffered through a 32-slot mpsc channel per
//!   connection, then written to the salvo socket from a dedicated write
//!   task. Streaming handler outputs (`WsHandlerOutput::Stream`) run as
//!   spawned tasks that are aborted when the read loop ends.
//!
//! ## Graceful shutdown
//!
//! `app.close()` (or any caller of `HttpAdapter::close` /
//! `WebSocketAdapter::close`) flips a shared `tokio::sync::watch` channel,
//! which a per-server task observes and forwards to
//! `salvo::Server::handle().stop_graceful(None)`. Both the HTTP server and
//! every separate-port WS listener stop in response to a single signal.
//!
//! ## Panics
//!
//! `bind` for either adapter panics on bind failure (port in use, permission
//! denied, etc.) — the framework's intentional fail-fast behavior. Adapter
//! types themselves never panic at runtime; transport-level errors flow
//! through `tracing::debug` and `tracing::warn` events.

mod salvo_adapter;
mod salvo_websocket_adapter;
pub(crate) mod tokio_sender;

pub use salvo_adapter::SalvoAdapter;
pub use tokio_sender::TokioSender;

pub use toni::{HttpAdapter, WebSocketAdapter};
