//! gRPC transport adapter for the [Toni](https://github.com/monterxto/toni-rs) framework.
//!
//! Drives a [`tonic`](https://docs.rs/tonic) server through Toni's bind /
//! serve / drain lifecycle. Services declared with the framework's
//! `#[grpc_service]` + `#[grpc_methods]` macros are discovered via DI and
//! wrapped with the enhancer pipeline (guards / interceptors / error
//! handlers + panic recovery) before being handed to tonic — there is no
//! manual `*Server::new(handler)` step in user code.
//!
//! # Minimal example
//!
//! ```ignore
//! use std::net::SocketAddr;
//! use toni::ToniFactory;
//! use toni_macros::{grpc_methods, grpc_service, injectable, module};
//!
//! mod orders_pb {
//!     tonic::include_proto!("toni_examples.orders");
//! }
//! use orders_pb::orders_server::{Orders, OrdersServer};
//!
//! #[grpc_service(pub struct OrdersGrpcService {})]
//! impl OrdersGrpcService {
//!     pub fn new() -> Self { Self {} }
//! }
//!
//! #[grpc_methods]
//! #[tonic::async_trait]
//! impl Orders for OrdersGrpcService { /* methods */ }
//!
//! #[module(providers: [OrdersGrpcService])]
//! struct AppModule;
//!
//! #[tokio::main(flavor = "current_thread")]
//! async fn main() {
//!     let local = tokio::task::LocalSet::new();
//!     local.run_until(async move {
//!         let addr: SocketAddr = "127.0.0.1:50051".parse().unwrap();
//!         let mut app = ToniFactory::create(AppModule::module_definition()).await;
//!         app.use_grpc_adapter(toni_grpc::GrpcAdapter::new(addr)).unwrap();
//!         app.start().await.unwrap();
//!     }).await;
//! }
//! ```
//!
//! # Mixing DI-registered and manually-added services
//!
//! [`GrpcAdapter::add_service`] accepts any tonic-generated service handle,
//! so a non-DI service can sit alongside `#[grpc_service]`-registered ones:
//!
//! ```ignore
//! let adapter = toni_grpc::GrpcAdapter::new(addr)
//!     .add_service(SomeOtherServer::new(other_handler));
//! app.use_grpc_adapter(adapter)?;
//! ```
//!
//! Services added this way do not get the enhancer pipeline — they pass
//! straight through to tonic. Services registered via DI go through guards,
//! interceptors, error handlers, and the panic catcher.
//!
//! # Drain timeout
//!
//! On shutdown the framework calls the adapter's `close`, which signals
//! tonic and starts a drain timer (default 10 s). When the timer elapses
//! with in-flight calls still running, the serve future is dropped — open
//! connections close, streaming clients see `UNAVAILABLE`. Configure with
//! [`GrpcAdapter::with_drain_timeout`]; pass `None` to wait without bound.
//!
//! # Tracing
//!
//! Every dispatched method runs inside a `tracing::info_span!("rpc.request",
//! transport = "grpc", pattern = …, peer = …)` span, so any event the user
//! handler emits inherits those fields without having to thread context
//! through.
//!
//! See the crate's [README](https://docs.rs/toni-grpc) for the full enhancer
//! API (guards / interceptors / error handlers / panic recovery) and a
//! runnable end-to-end example.

mod grpc_adapter;
mod tracing_layer;

pub use grpc_adapter::GrpcAdapter;
