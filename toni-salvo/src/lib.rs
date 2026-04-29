//! # toni-salvo
//!
//! Salvo adapter for the Toni framework.
//!
//! Provides an implementation of Toni's `HttpAdapter` trait for the Salvo web framework,
//! including same-port WebSocket upgrade support.

mod salvo_adapter;
mod salvo_websocket_adapter;
pub(crate) mod tokio_sender;

pub use salvo_adapter::SalvoAdapter;
pub use tokio_sender::TokioSender;

pub use toni::HttpAdapter;
