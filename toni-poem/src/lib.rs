//! # toni-poem
//!
//! [Poem](https://crates.io/crates/poem) adapter for the toni framework.
//! Implements both `HttpAdapter` and `WebSocketAdapter`, so a single adapter
//! type covers HTTP routes, same-port WebSocket upgrades, and separate-port
//! WebSocket servers.
//!
//! ## Usage
//!
//! ```ignore
//! use toni_poem::PoemAdapter;
//!
//! #[tokio::main]
//! async fn main() {
//!     let mut app = ToniFactory::new()
//!         .create_with(AppModule::module_definition())
//!         .await;
//!     app.use_http_adapter(PoemAdapter::new(), 3000, "127.0.0.1").unwrap();
//!     // Only needed for `#[websocket_gateway(port = N)]` gateways.
//!     app.use_websocket_adapter(PoemAdapter::new()).unwrap();
//!     app.start().await.unwrap();
//! }
//! ```
//!
//! ## Routing
//!
//! Toni and poem agree on `:param` syntax, so handler routes pass through
//! unchanged. The adapter merges every `(method, path)` pair into a single
//! `RouteMethod` per path before mounting — poem panics if `Route::at` is
//! called twice with the same path, so multi-method endpoints accumulate
//! before reaching the router. A `*toni_fallback` wildcard catches anything
//! unmatched and runs the toni 404 through the global middleware chain.
//!
//! Path parameters are recovered via `Request::path_params::<HashMap>` and
//! installed in toni's `PathParams` extension so the `Path<T>` extractor
//! reads them downstream.
//!
//! ## HTTP methods
//!
//! Every standard method has a direct mapping: `GET`, `POST`, `PUT`,
//! `DELETE`, `PATCH`, `HEAD`, `OPTIONS`, `TRACE`, `CONNECT`. Poem's
//! `RouteMethod` exposes a setter for each, so dispatch is exact-match —
//! there's no method-agnostic fallback to work around.
//!
//! ## Request bodies
//!
//! Poem's `Body` is a thin newtype around `http_body_util::BoxBody<Bytes,
//! IoError>`, the same family toni already uses internally. The adapter
//! re-erases it as toni's `RequestBoxBody` and hands it through unbuffered.
//! `BodyStream` consumers see frames as they arrive; `Bytes`, `Json`, and
//! friends call `RequestBody::collect` to drain on demand.
//!
//! ## Response bodies
//!
//! Toni's [`Body::stream`](toni::http_helpers::Body::stream) is forwarded
//! into a `poem::Body` via `BoxBody`, so chunks reach the client
//! incrementally. SSE and other long-lived streaming responses work
//! without buffering.
//!
//! ## WebSockets
//!
//! - **Same-port** upgrades go through poem's `WebSocket` extractor on the
//!   HTTP listener. Sub-protocols (`Sec-WebSocket-Protocol`) are negotiated
//!   by echoing the first protocol the client offers.
//! - **Separate-port** gateways (`#[websocket_gateway(port = N)]`) bind
//!   their own poem server on the requested port. `port = 0` lets the OS
//!   assign a distinct listener.
//! - Outbound messages are buffered through a 32-slot mpsc channel per
//!   connection, then written to the poem socket from a dedicated write
//!   task. Streaming handler outputs (`WsHandlerOutput::Stream`) run as
//!   spawned tasks that are aborted when the read loop ends.
//!
//! ## Graceful shutdown
//!
//! Poem exposes `Server::run_with_graceful_shutdown(endpoint, signal,
//! timeout)` natively, so a single `tokio::sync::watch` channel feeds both
//! the HTTP and any separate-port WS servers. `app.close()` flips the
//! channel and every server stops in response to the same signal.
//!
//! ## Bind failures
//!
//! Bind failures (port in use, permission denied, etc.) surface as errors
//! from `app.start()` — the framework's intentional fail-fast behavior.
//! Adapter types themselves never panic at runtime; transport-level errors
//! flow through `tracing::debug` and `tracing::warn` events.

mod poem_adapter;
mod poem_websocket_adapter;
pub(crate) mod tokio_sender;

pub use poem_adapter::PoemAdapter;
pub use tokio_sender::TokioSender;

pub use toni::{HttpAdapter, WebSocketAdapter};
