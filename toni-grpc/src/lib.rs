//! gRPC transport adapter for the Toni framework.
//!
//! Wraps `tonic::transport::Server` and plugs into the framework's lifecycle
//! orchestration via the `GrpcAdapter` trait. This crate is the bind/serve/
//! drain layer only — service registration and dispatch are handled by tonic
//! and the user's `tonic-build`-generated service traits.
//!
//! # Example
//!
//! ```ignore
//! use std::net::SocketAddr;
//! use toni_grpc::GrpcAdapter;
//!
//! let addr: SocketAddr = "127.0.0.1:50051".parse()?;
//! let adapter = GrpcAdapter::new(addr).add_service(my_service_server);
//! app.use_grpc_adapter(adapter)?;
//! ```

mod grpc_adapter;
mod tracing_layer;

pub use grpc_adapter::GrpcAdapter;
