//! gRPC's pre-dispatch stage (transports DESIGN §6.2, X23): `ulo_http::PreDispatch<Grpc>`, scoped
//! by method path, run by the server around its dispatcher through `ulo_http::stage`. A response an
//! entry answers with a non-200 HTTP status is translated with `code_for_http`.

use crate::transport::Grpc;

/// The gRPC pre-dispatch stage's entries one module declares:
///
/// ```ignore
/// m.meta::<ulo_grpc::PreDispatch>()
///     .layer(TraceLayer::new_for_grpc())
///     .apply_for::<ApiKeyAuth>(["/users.v1.UserService/*"]);
/// ```
///
/// A metadata key apart from HTTP's `ulo_http::PreDispatch`, so an entry declared for one never
/// runs on the other.
pub type PreDispatch = ulo_http::PreDispatch<Grpc>;
