#[path = "adapter/mod.rs"]
pub mod adapter;
mod application_context;
pub mod builtin_module;
pub mod context;
mod error;
pub use context::{
    CancellationToken, GrpcContext, HandlerContext, HttpContext, RpcContext, WsContext,
};
#[doc(hidden)]
pub mod __construct;
#[doc(hidden)]
pub mod __detect;
#[doc(hidden)]
pub mod __lifecycle;
#[doc(hidden)]
pub mod __route;
pub mod di;
pub mod errors;
pub mod extractors;
pub mod grpc_status;
pub use grpc_status::{GrpcCode, GrpcStatus};
pub mod grpc_runtime;
pub mod http_helpers;
pub mod injector;
pub mod middleware;
pub mod module_helpers;
pub mod panic_recovery;
pub mod provider_scope;
mod request;
mod router;
pub mod rpc;
mod scanner;
mod structs_helpers;
pub mod toni_application;
pub mod toni_factory;
pub mod traits_helpers;
pub mod websocket;

// Re-exported for use in macro-generated code — not part of the public API.
#[doc(hidden)]
pub use tracing;

// Re-exports for adapter crates
pub use adapter::{
    AdapterContext, GrpcAdapter, GrpcLifecycleHandle, HttpAdapter, HttpLifecycleHandle,
    MessageCallbackResult, RequestHandler, RpcAdapter, RpcClientTransport, RpcLifecycleHandle,
    RpcMessageCallbacks, ServerHandle, WebSocketAdapter, WsConnectionCallbacks, WsLifecycleHandle,
};
pub use http_helpers::{
    Body, BoxBody, HttpMethod, HttpRequest, HttpResponse, HttpResponseBuilder, IntoResponse,
    RequestBody, RequestBoxBody, RequestPart, RouteMetadata, Sse, SseEvent, sse,
};
pub use injector::InstanceWrapper;
pub use rpc::{RpcCallInfo, RpcClient, RpcClientError, RpcControllerTrait, RpcData, RpcError};
pub use websocket::{
    BroadcastError, BroadcastModule, BroadcastService, BroadcastTarget, ClientId, DisconnectReason,
    GatewayEnhancers, GatewayHandlerEnhancers, GatewayTrait, GatewayWrapper, RoomId, SendError,
    TrySendError, WsClient, WsError, WsHandlerOutput, WsHandlerResult, WsHandshake, WsMessage,
    WsSink,
};

// Re-export built-in providers
pub use request::{Request, RequestFactory};

// Re-export ModuleRef for dynamic DI resolution
pub use injector::{IntoToken, ModuleRef};

pub use application_context::ToniApplicationContext;

// Re-export dependencies used in macro-generated code
// This allows users to only depend on `toni` without needing to add these explicitly
pub use async_trait::async_trait;
pub use rustc_hash::FxHashMap;

// Re-export provider scope
pub use provider_scope::ProviderScope;

pub use traits_helpers::{HttpProviderContext, ProviderContext, RequestCache};

pub use error::{BindError, InitResult};
pub use errors::{
    Cancelled, Error, ErrorKind, GuardRejection, HttpError, MiddlewareFailure, PanicRecovered,
    PipelineSegment,
};

// Re-export trait so users wont have to import manually
pub use extractors::{BodyStream, FromRequest, FromRequestParts};

// Re-export macros
pub use toni_macros::*;

pub use module_helpers::DynamicModule;
pub use toni_application::{BoundAdapters, ShutdownHandle, ToniApplication};
pub use toni_factory::ToniFactory;

#[cfg(feature = "tower-compat")]
pub mod tower_compat;
#[cfg(feature = "tower-compat")]
pub use tower_compat::TowerLayer;
