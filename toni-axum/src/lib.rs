//! # toni-axum
//!
//! Axum adapter for the Toni framework.
//!
//! This crate provides an implementation of Toni's `HttpAdapter` and `WebSocketAdapter` traits
//! for the Axum web framework.
//!
//! ## Usage
//!
//! ```ignore
//! use toni_axum::AxumAdapter;
//!
//! #[tokio::main]
//! async fn main() {
//!     let mut app = ToniFactory::new()
//!         .create_with(AppModule)
//!         .await;
//!     app.use_http_adapter(AxumAdapter::new(), ("127.0.0.1", 3000)).unwrap();
//!     // Only needed for `#[websocket_gateway(port = N)]` gateways.
//!     app.use_websocket_adapter(AxumAdapter::new()).unwrap();
//!     app.start().await.unwrap();
//! }
//! ```

mod axum_adapter;
mod axum_websocket_adapter;
pub(crate) mod tokio_sender;

pub use axum_adapter::AxumAdapter;
pub use tokio_sender::TokioSender;

pub use toni::{HttpAdapter, WebSocketAdapter};
