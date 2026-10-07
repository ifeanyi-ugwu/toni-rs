//! The proto `build.rs` compiles through `ulo-build`, for the tests beside it: its messages, its
//! client, its method markers and its file descriptor set.
//!
//! The tests in `tests/` serve and call it. `tests/ui` holds the programs the attributes must
//! refuse to compile.

/// The package `codegen.v1`, a service with a method of each call shape and a
/// second service beside it.
pub mod pb {
    ulo_grpc::include_proto!("codegen.v1");
}

/// The package `codegen.probe.v1`: well-known types, the reply forms around a stream and a
/// message, deadlines, and a service behind enhancers.
pub mod probe {
    ulo_grpc::include_proto!("codegen.probe.v1");
}

/// A proto with no `package`, which prost writes to `_.rs`.
pub mod bare {
    ulo_grpc::include_proto!("_");
}
