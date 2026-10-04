//! The gRPC transport (transports DESIGN §6): handlers bound to the method markers `ulo-build`
//! generates, one `:path` dispatcher over tonic's `server::Grpc`, an HTTP/2 [`Server`] over
//! `ulo-hyper-serve`, the pre-dispatch stage as [`PreDispatch`], status and details mapping,
//! health, reflection, and client modules.
//!
//! ```ignore
//! // build.rs
//! ulo_build::configure().compile(&["proto/users.proto"], &["proto"])?;
//!
//! // src/pb.rs
//! ulo_grpc::include_proto!("users.v1");
//!
//! #[routes]
//! #[guards(grpc = TokenGuard)]
//! impl UsersGrpc {
//!     #[ulo_grpc::method(pb::user_service::GetUser)]
//!     async fn get_user(&self, req: Message<pb::GetUserRequest>) -> Result<Response<pb::User>, UserError> {
//!         let user = self.users.find(req.0.id).await?;
//!         Ok(Response::new(user.into()).metadata("x-cache", "miss"))
//!     }
//! }
//! ```
//!
//! tonic's generated service trait is never implemented: the attribute checks the handler's
//! signature against the marker's types and shape, and the dispatcher calls it by path. gRPC
//! serves on its own port; nothing routes an `application/grpc` request arriving on the HTTP
//! server's port to it.

mod client;
mod dispatch;
mod extract;
mod health;
mod method;
mod pre_dispatch;
mod reflection;
mod server;
mod status;
mod transport;

#[doc(hidden)]
pub mod __private;

pub use client::{ClientTls, GrpcClient, GrpcClientModule, GrpcEndpoint, outgoing};
pub use extract::{Message, Request, Streaming};
pub use health::GrpcHealth;
pub use method::{Method, Shape};
pub use pre_dispatch::PreDispatch;
pub use server::Server;
pub use status::{code_for, code_for_http, to_status};
pub use transport::{Grpc, GrpcCx, GrpcMetadata, PeerAddr, Reply, Response};
pub use ulo_grpc_macros::method;

/// The code `ulo-build` generated for the protobuf package `$package`: its messages, its clients
/// and the method markers, with the encoded file descriptor set as `FILE_DESCRIPTOR_SET` when the
/// build step wrote one: `ulo_grpc::include_proto!("users.v1");`.
#[macro_export]
macro_rules! include_proto {
    ($package:literal) => {
        include!(concat!(env!("OUT_DIR"), "/", $package, ".rs"));
    };
}
