//! The `:path` dispatcher (transports DESIGN §6.1): one tower service routing each call to its
//! marker's handler through `tonic::server::Grpc::new(ProstCodec<M::Request, M::Response>)` and its
//! `unary`, `server_streaming`, `client_streaming` and `streaming` methods, each over a type
//! implementing the matching shape-service trait. A path off the table answers UNIMPLEMENTED,
//! offered to no error handler. `grpc-timeout` is parsed here into the execution's deadline.
