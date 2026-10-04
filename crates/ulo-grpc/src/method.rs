//! The method marker `ulo-build` writes per RPC method, beside tonic's output, and a handler binds
//! to (transports DESIGN §6.1).

pub use ulo::Shape;

/// One RPC method, as a generated marker type:
///
/// ```ignore
/// pub struct GetUser;
/// impl ulo_grpc::Method for GetUser {
///     const PATH: &'static str = "/users.v1.UserService/GetUser";
///     const SHAPE: Shape = Shape::Unary;
///     type Request = GetUserRequest;
///     type Response = User;
/// }
/// ```
///
/// `#[ulo_grpc::method(Marker)]` checks the handler's signature against `Request`, `Response` and
/// `SHAPE` through trait bounds, so a mismatch fails to compile. Two handlers for one `PATH` are a
/// `Configure` error.
pub trait Method: Send + Sync + 'static {
    /// The HTTP/2 `:path` the method is dialled on: `/<package>.<Service>/<Method>`.
    const PATH: &'static str;
    const SHAPE: Shape;
    type Request: prost::Message + Default + Send + 'static;
    type Response: prost::Message + Default + Send + 'static;
}
