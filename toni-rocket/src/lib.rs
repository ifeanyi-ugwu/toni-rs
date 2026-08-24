//! # toni-rocket
//!
//! [Rocket](https://crates.io/crates/rocket) adapter for the toni framework.
//! Implements `HttpAdapter` with same-port WebSocket upgrade via
//! [`rocket_ws`](https://crates.io/crates/rocket_ws). Separate-port
//! WebSocket is intentionally not implemented — pair toni-rocket with a
//! dedicated WebSocket adapter (e.g. `toni-tungstenite`) when you need it.
//!
//! ## Usage
//!
//! ```ignore
//! use toni_rocket::RocketAdapter;
//!
//! #[tokio::main]
//! async fn main() {
//!     let mut app = ToniFactory::new()
//!         .create_with(AppModule)
//!         .await
//!         .unwrap();
//!     app.use_http_adapter(RocketAdapter::new(), ("127.0.0.1", 3000)).unwrap();
//!     app.start().await.unwrap();
//! }
//! ```
//!
//! ## Routing
//!
//! Routing is internal: the adapter mounts one catch-all rocket route per
//! method and dispatches through its own `match_route` over the registered
//! route table, matching toni's `{param}` / `:param` / `*tail` syntaxes.
//! Captured path parameters are stashed in toni's `PathParams` extension so
//! the `Path<T>` extractor reads them downstream.
//!
//! ## HTTP methods
//!
//! Every standard method has a direct mapping: `GET`, `POST`, `PUT`,
//! `DELETE`, `PATCH`, `HEAD`, `OPTIONS`, `TRACE`, `CONNECT`. Rocket's
//! `Method` enum exposes a variant for each, so dispatch is exact-match.
//!
//! ## Request bodies
//!
//! Rocket's `Data<'r>` is lifetime-bound to the request — it can't outlive
//! the handler. The adapter reads the entire body into `Bytes` inside the
//! handler future and hands toni a buffered `RequestBody`, capped at 32 MiB
//! by default. This is a meaningful difference from toni-axum/poem/salvo,
//! which stream request bodies frame-by-frame; toni's `BodyStream`
//! extractor still works, it just sees the full payload as a single chunk.
//!
//! ## Response bodies
//!
//! Toni's [`Body::stream`](toni::http_helpers::Body::stream) is bridged
//! into rocket's `streamed_body` via `tokio_util::io::StreamReader`, so
//! chunks reach the client incrementally. Buffered bodies use
//! `sized_body` for an accurate Content-Length without forcing chunked
//! transfer.
//!
//! ## WebSockets
//!
//! Same-port upgrades go through `rocket_ws::WebSocket`. Routes registered
//! via `register_ws_route` are mounted as `GET` (matching RFC 6455) and the upgrade
//! is performed inside the handler. The adapter does not implement
//! `WebSocketAdapter`, so `#[websocket_gateway(port = N)]` gateways will
//! fail registration — pair toni-rocket with `toni-tungstenite` (or
//! another `WebSocketAdapter`) for separate-port WS.
//!
//! ## Graceful shutdown
//!
//! Rocket exposes a `Shutdown` handle natively. The adapter ignites the
//! rocket explicitly to obtain the handle, then forwards toni's
//! `tokio::sync::watch` shutdown signal to `Shutdown::notify()`. The
//! `launch()` future resolves once rocket has drained in-flight
//! connections.
//!
//! ## Pre-bound listeners
//!
//! Only address targets are supported: `BindTarget::Listener` is refused with
//! an error at `app.bind()`. Rocket binds inside `launch()` from figment
//! configuration and accepts no externally constructed listener, so socket
//! activation and socket-preserving restarts need one of the other HTTP
//! adapters.
//!
//! ## Bound-address discovery
//!
//! Rocket fuses bind and serve into `Rocket::launch()`, with no public hook
//! that returns the OS-assigned address before the serve loop starts. The
//! adapter installs an `AdHoc::on_liftoff` fairing that captures the bound
//! `SocketAddr` once rocket has reached the `Orbit` phase and forwards it
//! through a `oneshot` channel back to `listen()`. Tests using `port = 0`
//! recover the assigned port through this path.
//!
//! ## Panics
//!
//! Bind failures (port in use, permission denied, etc.) propagate as
//! `Result::Err` from the `listen` future. The adapter never panics at
//! runtime; transport-level errors flow through `tracing::debug` and
//! `tracing::warn` events.

mod rocket_adapter;
mod rocket_websocket_adapter;
pub(crate) mod tokio_sender;

pub use rocket_adapter::RocketAdapter;
pub use tokio_sender::TokioSender;

pub use toni::HttpAdapter;
