// An item reachable from nowhere is still public API: it holds a name, appears in the rustdoc
// index, and cannot change without a major version. Denied rather than warned because CI's check
// job does not pass `-D warnings`, so a warning here would accumulate unnoticed.
#![deny(unreachable_pub)]

#[doc(hidden)]
pub mod __construct;
#[doc(hidden)]
pub mod __detect;
#[doc(hidden)]
pub mod __dispatch;
#[doc(hidden)]
pub mod __enhancer;
#[doc(hidden)]
pub mod __grpc;
#[doc(hidden)]
pub mod __http;
#[doc(hidden)]
pub mod __lifecycle;
#[doc(hidden)]
pub mod __rpc;
#[doc(hidden)]
pub mod __ws;
mod application;
mod application_context;
pub mod context;
pub mod di;
pub mod dispatch;
pub mod enhancer;
mod error;
pub mod errors;
pub mod extract;
mod factory;
pub mod grpc;
pub mod http;
mod panic_recovery;
pub mod rpc;
mod server_lifecycle;
pub mod spi;
mod startup_check;
pub mod ws;

// Re-exported for use in macro-generated code — not part of the public API.
#[doc(hidden)]
pub use tracing;

// Public re-export: macro-generated code builds error bodies with it, and
// `Body::json` traffics in its `Value` type, so consumers need it in scope
// without declaring their own dependency.
pub use serde_json;

// Re-exported because public signatures name their types: `multer::Field`
// and `multer::Error` through `Multipart`, `validator::ValidationErrors` and
// `Validate` through `Validated`'s error and `ValidatableExtractor`, and
// `bytes::Bytes` through `BodyStream` and the HTTP body. `Field` is also
// `ulo::http::extract::Field`, the path to write for it. Deriving `Validate`
// still needs `validator` as a direct dependency, since the derive names
// `::validator`.
pub use bytes;
pub use multer;
pub use validator;

// Re-exports for adapter crates

// Re-export built-in providers

// Re-export ModuleRef for dynamic DI resolution

// Re-export dependencies used in macro-generated code
// This allows users to only depend on `ulo` without needing to add these explicitly
pub use async_trait::async_trait;
/// Re-exported for generated code: a streaming gRPC handler's reply is boxed
/// and mapped in the expansion, which needs a `Stream` path the user's crate
/// can name without depending on `futures` itself.
pub use futures;
pub use rustc_hash::FxHashMap;

// Re-export provider scope

// Re-export trait so users wont have to import manually

// Re-export macros
pub use ulo_macros::*;

// What a `main` writes. Everything else lives in the module that owns it, and `prelude` collects
// what an application reaches for whatever it serves.
pub use application::{BoundAdapters, ShutdownHandle, UloApplication};
pub use application_context::UloApplicationContext;
pub use error::StartupError;
pub use errors::{Error, ErrorKind};
pub use factory::UloFactory;
pub use startup_check::StartupCheck;

/// What an application writes whatever it serves.
///
/// `use ulo::prelude::*` brings the bootstrap, the macros, the DI vocabulary, the enhancer traits
/// and the extraction traits — everything that means the same thing on HTTP, RPC, WebSocket and
/// gRPC. What a transport adds is its own: `ulo::http::{Body, HttpResponse}`,
/// `ulo::rpc::RpcData`, `ulo::ws::WsMessage`, `ulo::grpc::GrpcStatus`.
///
/// The prelude deliberately carries no transport's types. A framework that put HTTP's in here
/// would be saying HTTP is the default and the rest are extras, which is not how anything else in
/// the crate is arranged.
///
/// ```rust
/// use ulo::http::Body;
/// use ulo::prelude::*;
///
/// #[controller("/health")]
/// pub struct Health {}
///
/// #[routes]
/// impl Health {
///     #[get("/")]
///     async fn check(&self) -> Body {
///         Body::text("ok")
///     }
/// }
///
/// #[module(controllers: [Health])]
/// impl AppModule {}
/// ```
pub mod prelude {
    pub use crate::{BoundAdapters, ShutdownHandle, StartupCheck, StartupError, UloApplication};
    pub use crate::{UloApplicationContext, UloFactory};

    pub use crate::di::{CheckedModule, DynamicModule, Extension, InitResult, ModuleIdentity};
    pub use crate::di::{Execution, ModuleMetadata};
    pub use crate::di::{Key, ModuleRef, ProviderScope};
    pub use crate::key;

    pub use crate::dispatch::Items;
    pub use crate::enhancer::{ChainError, ErrorHandler, Guard, Interceptor, InterceptorNext};
    pub use crate::errors::{Error, ErrorKind};
    pub use crate::extract::{FromContext, Payload, Validated, take_body};

    pub use crate::async_trait;
    pub use ulo_macros::*;
}
