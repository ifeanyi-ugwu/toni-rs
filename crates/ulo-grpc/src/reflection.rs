//! `grpc.reflection.v1` and `v1alpha` from the generated descriptor sets, through
//! `tonic_reflection::server::Builder` and its `build_v1` and `build_v1alpha` (transports DESIGN
//! §6.2). Added to the path table in debug builds and behind `Server::reflection(true)` in release
//! builds.
