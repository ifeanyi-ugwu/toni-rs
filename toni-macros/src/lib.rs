extern crate proc_macro2;

use proc_macro::TokenStream;

mod app_error_macro;
mod catch_macro;
mod config_macro;
mod controller_macro;
mod enhancer;
mod gateway_macro;
mod grpc_macro;
mod markers_params;
mod middleware_macro;
mod module_macro;
mod provider_macro;
mod provider_variants;
mod rpc_macro;
mod shared;
mod utils;

#[proc_macro_attribute]
pub fn module(attr: TokenStream, item: TokenStream) -> TokenStream {
    module_macro::module_struct::module(attr, item)
}

/// Field-injection provider — the way to declare a DI provider. Place it on the struct:
/// `#[inject]` fields are dependencies, `#[default(expr)]` fields are owned state. The macro adds
/// the `Clone` impl the container needs, so the struct carries no derive ceremony.
///
/// Arguments override defaults:
/// - `#[injectable(scope = "request")]` / `"transient"` — default is singleton.
/// - `#[injectable(init = "new")]` — assemble via `Self::new(inject_fields…)` instead of a struct
///   literal. (Usually unnecessary — prefer `#[new]` on the constructor method, which also injects
///   parameters that aren't stored fields.)
///
/// Construction logic (`#[new]`) and lifecycle hooks (`#[on_module_init]`, …) live on the struct's `impl`.
///
/// ```ignore
/// #[injectable(scope = "request")]
/// pub struct UserService {
///     #[inject] repo: UserRepo,
/// }
/// ```
#[proc_macro_attribute]
pub fn injectable(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attr = proc_macro2::TokenStream::from(attr);
    let item = proc_macro2::TokenStream::from(item);
    let output = provider_macro::provider_attr::handle_provider(attr, item);
    proc_macro::TokenStream::from(output.unwrap_or_else(|e| e.to_compile_error()))
}

/// HTTP controller — declared like `#[injectable]`, on the struct. `#[inject]` fields are
/// dependencies, `#[default(expr)]` fields are owned state, `#[controller("/prefix", scope = "…")]`
/// sets the route prefix and scope. The route handlers live in a sibling `#[routes] impl` block.
///
/// ```ignore
/// #[controller("/users")]
/// pub struct UsersController { #[inject] svc: UserService }
///
/// #[routes]
/// impl UsersController {
///     #[get("/")] async fn list(&self) -> impl IntoResponse { /* … */ }
/// }
/// ```
#[proc_macro_attribute]
pub fn controller(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attr = proc_macro2::TokenStream::from(attr);
    let item = proc_macro2::TokenStream::from(item);
    let output = controller_macro::controller_attr::handle_controller(attr, item);
    proc_macro::TokenStream::from(output.unwrap_or_else(|e| e.to_compile_error()))
}

/// Route handlers for a `#[controller]` struct. Place it on the struct's `impl` block; `#[get]`,
/// `#[post]`, … methods become routes. Pairs with `#[controller("/prefix")]` on the struct.
#[proc_macro_attribute]
pub fn routes(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let item = proc_macro2::TokenStream::from(item);
    let output = controller_macro::routes_attr::handle_routes(item);
    proc_macro::TokenStream::from(output.unwrap_or_else(|e| e.to_compile_error()))
}

#[proc_macro_attribute]
pub fn get(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}
#[proc_macro_attribute]
pub fn post(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}
#[proc_macro_attribute]
pub fn put(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}
#[proc_macro_attribute]
pub fn delete(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}

/// Applies guards to route handlers or controllers for request authorization.
///
/// Guards execute before the route handler and can block requests based on custom logic.
/// Multiple guards can be specified and execute in the order listed.
///
/// # Syntax
///
/// - **Type name only** - Requires the guard to be registered in DI container:
///   ```rust,ignore
///   #[use_guards(AuthGuard)]
///   ```
///
/// - **Struct literal** - Directly instantiates the guard:
///   ```rust,ignore
///   #[use_guards(SimpleGuard{})]
///   #[use_guards(AdminGuard { role: "admin" })]
///   ```
///
/// - **Constructor call** - Directly calls the constructor:
///   ```rust,ignore
///   #[use_guards(RoleGuard::new("admin"))]
///   ```
///
/// # Examples
///
/// **Method-level guards:**
/// ```rust,ignore
/// #[use_guards(AuthGuard{}, RoleGuard::new("admin"))]
/// #[get("/admin")]
/// fn admin_panel(&self, req: HttpRequest) -> HttpResponse {
///     // Only accessible to authenticated admin users
/// }
/// ```
///
/// **Controller-level guards (applies to all methods):**
/// ```rust,ignore
/// #[controller("/api", pub struct MyController{})]
/// #[use_guards(AuthGuard{})]
/// impl MyController {
///     // All methods require authentication
/// }
/// ```
///
/// # Execution Order
///
/// Guards execute in hierarchical order:
/// 1. Global guards (registered via `ToniFactory`)
/// 2. Controller-level guards
/// 3. Method-level guards
///
/// Within each level, guards execute in the order specified.
#[proc_macro_attribute]
pub fn use_guards(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}

/// Applies interceptors to route handlers or controllers for cross-cutting concerns.
///
/// Interceptors wrap request/response handling, allowing you to execute logic before and after
/// the route handler. Common uses include logging, timing, transformation, and caching.
///
/// # Syntax
///
/// - **Type name only** - Requires the interceptor to be registered in DI container:
///   ```rust,ignore
///   #[use_interceptors(LoggingInterceptor)]
///   ```
///
/// - **Struct literal** - Directly instantiates the interceptor:
///   ```rust,ignore
///   #[use_interceptors(TimingInterceptor{})]
///   #[use_interceptors(CacheInterceptor { ttl: Duration::from_secs(60) })]
///   ```
///
/// - **Constructor call** - Directly calls the constructor:
///   ```rust,ignore
///   #[use_interceptors(CacheInterceptor::new(Duration::from_secs(60)))]
///   ```
///
/// # Examples
///
/// **Method-level interceptors:**
/// ```rust,ignore
/// #[use_interceptors(TimingInterceptor{}, LoggingInterceptor{})]
/// #[get("/users")]
/// fn find_all(&self, req: HttpRequest) -> HttpResponse {
///     // Request is logged and timed
/// }
/// ```
///
/// **Controller-level interceptors (applies to all methods):**
/// ```rust,ignore
/// #[use_interceptors(LoggingInterceptor{})]
/// #[controller("/api")]
/// impl MyController {
///     // All methods are logged
/// }
/// ```
///
/// # Execution Order
///
/// Interceptors execute in hierarchical order with nested "before" and "after" phases:
/// 1. Global interceptors (registered via `ToniFactory`)
/// 2. Controller-level interceptors
/// 3. Method-level interceptors
/// 4. Route handler executes
/// 5. Method-level interceptors (after phase, reverse order)
/// 6. Controller-level interceptors (after phase, reverse order)
/// 7. Global interceptors (after phase, reverse order)
#[proc_macro_attribute]
pub fn use_interceptors(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}

/// Applies pipes to route handlers or controllers for data transformation and validation.
///
/// Pipes process request data before it reaches the route handler. Common uses include
/// validation, transformation, sanitization, and parsing.
///
/// # Syntax
///
/// - **Type name only** - Requires the pipe to be registered in DI container:
///   ```rust,ignore
///   #[use_pipes(ValidationPipe)]
///   ```
///
/// - **Struct literal** - Directly instantiates the pipe:
///   ```rust,ignore
///   #[use_pipes(TransformPipe{})]
///   #[use_pipes(ValidationPipe { strict: true })]
///   ```
///
/// - **Constructor call** - Directly calls the constructor:
///   ```rust,ignore
///   #[use_pipes(ValidationPipe::new(strict_mode))]
///   ```
///
/// # Examples
///
/// **Method-level pipes:**
/// ```rust,ignore
/// #[use_pipes(ValidationPipe{}, TransformPipe{})]
/// #[post("/users")]
/// fn create_user(&self, req: HttpRequest) -> HttpResponse {
///     // Request data is validated and transformed
/// }
/// ```
///
/// **Controller-level pipes (applies to all methods):**
/// ```rust,ignore
/// #[use_pipes(ValidationPipe{})]
/// #[controller("/api")]
/// impl MyController {
///     // All methods validate request data
/// }
/// ```
///
/// # Execution Order
///
/// Pipes execute in hierarchical order:
/// 1. Global pipes (registered via `ToniFactory`)
/// 2. Controller-level pipes
/// 3. Method-level pipes
///
/// Within each level, pipes execute in the order specified.
#[proc_macro_attribute]
pub fn use_pipes(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}

/// Applies error handlers to route handlers or controllers for custom error processing.
///
/// Error handlers catch errors from route handlers and return custom HTTP responses.
/// They follow a chain-of-responsibility pattern where specialized handlers can pass
/// errors to more generic handlers by returning None.
///
/// # Syntax
///
/// - **Type name only** - Requires the error handler to be registered in DI container:
///   ```rust,ignore
///   #[use_error_handlers(CustomErrorHandler)]
///   ```
///
/// - **Struct literal** - Directly instantiates the error handler:
///   ```rust,ignore
///   #[use_error_handlers(ValidationErrorHandler{})]
///   #[use_error_handlers(DatabaseErrorHandler { log_queries: true })]
///   ```
///
/// - **Constructor call** - Directly calls the constructor:
///   ```rust,ignore
///   #[use_error_handlers(TracingErrorHandler::new(level))]
///   ```
///
/// # Examples
///
/// **Method-level error handlers:**
/// ```rust,ignore
/// #[use_error_handlers(ValidationErrorHandler{}, DatabaseErrorHandler{})]
/// #[post("/users")]
/// fn create_user(&self, req: HttpRequest) -> Result<HttpResponse, HttpError> {
///     // Validation and database errors are handled by specialized handlers
/// }
/// ```
///
/// **Controller-level error handlers (applies to all methods):**
/// ```rust,ignore
/// #[use_error_handlers(CustomErrorHandler{})]
/// #[controller("/api")]
/// impl MyController {
///     // All methods use custom error handling
/// }
/// ```
///
/// # Execution Order
///
/// Error handlers execute in reverse hierarchical order (most specific first):
/// 1. Method-level error handlers (in order specified)
/// 2. Controller-level error handlers (in order specified)
/// 3. Global error handlers (registered via `ToniFactory`)
///
/// Each handler can return Some(response) to handle the error, or None to pass
/// to the next handler. If all handlers return None, a default 500 error is returned.
#[proc_macro_attribute]
pub fn use_error_handlers(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}

/// Attaches metadata to a route handler for use by guards, interceptors, or other enhancers.
///
/// Route metadata is stored once at startup and shared across all requests to the route.
/// Guards and interceptors can read this metadata via `context.metadata().unwrap().get::<T>()`.
///
/// # Usage
///
/// ```rust,ignore
/// // Define a metadata type
/// #[derive(Clone)]
/// pub struct Roles(pub Vec<&'static str>);
///
/// // Attach to route
/// #[set_metadata(Roles(vec!["admin", "moderator"]))]
/// #[get("/admin")]
/// fn admin_panel(&self) -> ToniBody { ... }
///
/// // Read in guard
/// #[async_trait]
/// impl Guard for RolesGuard {
///     async fn can_activate(&self, context: &Context) -> bool {
///         if let Some(Roles(required)) = context.metadata().unwrap().get::<Roles>() {
///             // Check user has required roles
///         }
///         true
///     }
/// }
/// ```
///
/// # Multiple Metadata
///
/// Multiple `#[set_metadata(...)]` attributes can be applied to the same route:
///
/// ```rust,ignore
/// #[set_metadata(Roles(vec!["user"]))]
/// #[set_metadata(RateLimit { max: 100, window: 60 })]
/// #[get("/api/data")]
/// fn get_data(&self) -> ToniBody { ... }
/// ```
#[proc_macro_attribute]
pub fn set_metadata(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}

/// Keeps `#[inject]` / `#[default]` valid as inert field attributes on structs that the
/// attribute-form macros (`#[injectable]`, `#[controller(…)]`, gateways, rpc/grpc) re-emit. Those
/// macros run their own provider codegen and only need the field attributes to stay parseable, so
/// this derive emits nothing.
#[proc_macro_derive(InjectFields, attributes(inject, default))]
pub fn derive_inject_fields(_input: TokenStream) -> TokenStream {
    TokenStream::new()
}

/// Marks the dependency-injected constructor of a `#[injectable]` struct.
///
/// Place it on a `fn name(deps…) -> Self` inside the struct's `impl`. Each parameter is resolved
/// from the DI container (by type, or `#[inject("TOKEN")]`) and passed in — so a dependency can be
/// a constructor argument without being a stored field, and the constructor can run real assembly
/// logic. Without `#[new]`, the provider builds the struct by field injection instead.
///
/// ```ignore
/// #[injectable]
/// pub struct Server { port: u16 }
///
/// impl Server {
///     #[new]
///     fn new(config: ConfigService) -> Self {   // config injected, not stored
///         Self { port: config.port() }
///     }
/// }
/// ```
#[proc_macro_attribute]
pub fn new(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let item = proc_macro2::TokenStream::from(item);
    let output = provider_macro::new_ctor::handle_new(item);
    proc_macro::TokenStream::from(output.unwrap_or_else(|e| e.to_compile_error()))
}

macro_rules! lifecycle_hook_macro {
    ($name:ident, $hook:expr, $doc:literal) => {
        #[doc = $doc]
        #[proc_macro_attribute]
        pub fn $name(_attr: TokenStream, item: TokenStream) -> TokenStream {
            let item = proc_macro2::TokenStream::from(item);
            let output = provider_macro::lifecycle_attr::handle_hook($hook, item);
            proc_macro::TokenStream::from(output.unwrap_or_else(|e| e.to_compile_error()))
        }
    };
}

lifecycle_hook_macro!(
    on_module_init,
    provider_macro::lifecycle_attr::Hook::OnInit,
    "Lifecycle hook on a `#[injectable]` struct: `async fn(&self) -> toni::InitResult`, run after the DI container is built. Returning `Err` aborts startup."
);
lifecycle_hook_macro!(
    on_application_bootstrap,
    provider_macro::lifecycle_attr::Hook::OnBootstrap,
    "Lifecycle hook: `async fn(&self) -> toni::InitResult`, run after all modules initialize, before the server accepts connections."
);
lifecycle_hook_macro!(
    on_module_destroy,
    provider_macro::lifecycle_attr::Hook::OnDestroy,
    "Lifecycle hook: `async fn(&self)`, run as the module is torn down during shutdown."
);
lifecycle_hook_macro!(
    before_application_shutdown,
    provider_macro::lifecycle_attr::Hook::BeforeShutdown,
    "Lifecycle hook: `async fn(&self, signal: Option<String>)`, run before shutdown begins."
);
lifecycle_hook_macro!(
    on_application_shutdown,
    provider_macro::lifecycle_attr::Hook::OnShutdown,
    "Lifecycle hook: `async fn(&self, signal: Option<String>)`, run as the application shuts down."
);

#[proc_macro_derive(Config, attributes(env, default, nested))]
pub fn derive_config(input: TokenStream) -> TokenStream {
    config_macro::derive_config(input)
}

/// Derive [`toni::Error`] from an annotated error type.
///
/// Tag the type (or each enum variant) with `#[error_kind(KIND)]`, where
/// `KIND` is a variant of `toni::ErrorKind`. Untagged variants fall back
/// to a top-level `#[error_kind(...)]` if present, otherwise to
/// `ErrorKind::Internal`.
///
/// ```ignore
/// use toni::Error;
///
/// #[derive(Debug, thiserror::Error, Error)]
/// enum BillingError {
///     #[error("invoice {0} not found")]
///     #[error_kind(NotFound)]
///     InvoiceNotFound(String),
///
///     #[error("card declined")]
///     #[error_kind(UnprocessableEntity)]
///     CardDeclined,
/// }
/// ```
#[proc_macro_derive(Error, attributes(error_kind))]
pub fn derive_error(input: TokenStream) -> TokenStream {
    app_error_macro::derive_app_error(input)
}

#[proc_macro]
pub fn provider_value(input: TokenStream) -> TokenStream {
    let input = proc_macro2::TokenStream::from(input);
    let output = provider_variants::handle_provider_value(input);
    proc_macro::TokenStream::from(output.unwrap_or_else(|e| e.to_compile_error()))
}

#[proc_macro]
pub fn provider_factory(input: TokenStream) -> TokenStream {
    let input = proc_macro2::TokenStream::from(input);
    let output = provider_variants::handle_provider_factory(input);
    proc_macro::TokenStream::from(output.unwrap_or_else(|e| e.to_compile_error()))
}

#[proc_macro]
pub fn provider_alias(input: TokenStream) -> TokenStream {
    let input = proc_macro2::TokenStream::from(input);
    let output = provider_variants::handle_provider_alias(input);
    proc_macro::TokenStream::from(output.unwrap_or_else(|e| e.to_compile_error()))
}

#[proc_macro]
pub fn provider_token(input: TokenStream) -> TokenStream {
    let input = proc_macro2::TokenStream::from(input);
    let output = provider_variants::handle_provider_token(input);
    proc_macro::TokenStream::from(output.unwrap_or_else(|e| e.to_compile_error()))
}

#[proc_macro]
pub fn provide(input: TokenStream) -> TokenStream {
    let input = proc_macro2::TokenStream::from(input);
    let output = provider_variants::handle_provide(input);
    proc_macro::TokenStream::from(output.unwrap_or_else(|e| e.to_compile_error()))
}

/// `#[catch(T)]` — escape hatch for runtime-selected error handling.
///
/// The framework's primary error path: a domain error type implements
/// [`toni::Error`] and the active transport renders it. `#[catch]` is for
/// cases that path doesn't reach — re-shaping framework events
/// (`GuardRejection`, etc.) per route or per controller, where one handler
/// claims an error and the chain falls through otherwise.
///
/// Lowers a free `async fn` into a unit struct whose `ErrorHandler<C, R>`
/// impl runs `error.downcast_ref::<T>()` and returns `None` on no match
/// (so the chain advances to the next handler).
///
/// ```ignore
/// use toni::{context::HttpContext, errors::HttpError, HttpResponse};
///
/// #[catch(HttpError)]
/// async fn render_4xx(err: &HttpError, _ctx: &HttpContext) -> HttpResponse {
///     // custom envelope for HttpError 4xx/5xx in this scope
///     err.to_response()
/// }
///
/// // Register on a controller / method:
/// #[use_error_handlers(render_4xx)]
/// ```
#[proc_macro_attribute]
pub fn catch(attr: TokenStream, item: TokenStream) -> TokenStream {
    catch_macro::catch(attr, item)
}

// ============================================================================
// WEBSOCKET GATEWAY MACROS
// ============================================================================

/// WebSocket gateway macro for defining WebSocket message handlers.
///
/// Similar to `#[controller]` but for WebSocket connections. Implements `GatewayTrait`
/// and handles WebSocket lifecycle events and message routing.
///
/// # Syntax
///
/// Placed on the struct, like `#[injectable]`. `#[inject]` fields are dependencies; the message
/// handlers live in a sibling `#[subscriptions]` impl.
///
/// - **Basic:** `#[websocket_gateway] pub struct Foo { ... }`
/// - **With path:** `#[websocket_gateway("/chat")] pub struct Foo { ... }`
/// - **With namespace:** `#[websocket_gateway("/chat", namespace = "lobby")] pub struct Foo { ... }`
///
/// # Examples
///
/// ```rust,ignore
/// #[websocket_gateway("/chat")]
/// pub struct ChatGateway {}
///
/// #[subscriptions]
/// impl ChatGateway {
///     #[subscribe_message("message")]
///     async fn handle_message(
///         &self,
///         client: WsClient,
///         message: WsMessage,
///     ) -> WsHandlerResult {
///         Ok(WsMessage::text("Echo: ...").into())
///     }
/// }
/// ```
#[proc_macro_attribute]
pub fn websocket_gateway(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attr = proc_macro2::TokenStream::from(attr);
    let item = proc_macro2::TokenStream::from(item);
    let output = gateway_macro::gateway_attr::handle_websocket_gateway(attr, item);
    proc_macro::TokenStream::from(output.unwrap_or_else(|e| e.to_compile_error()))
}

/// Message handlers for a `#[websocket_gateway]` struct. Place it on the struct's `impl`; the
/// `#[subscribe_message]` methods are scanned into the event router. Pairs with
/// `#[websocket_gateway("/path")]` on the struct. Connection hooks (`#[on_connect]` /
/// `#[on_disconnect]` / `#[after_init]`) are their own macros and need no `#[subscriptions]`.
#[proc_macro_attribute]
pub fn subscriptions(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let item = proc_macro2::TokenStream::from(item);
    let output = gateway_macro::subscriptions_attr::handle_subscriptions(item);
    proc_macro::TokenStream::from(output.unwrap_or_else(|e| e.to_compile_error()))
}

macro_rules! ws_connection_hook_macro {
    ($name:ident, $hook:expr, $doc:literal) => {
        #[doc = $doc]
        #[proc_macro_attribute]
        pub fn $name(_attr: TokenStream, item: TokenStream) -> TokenStream {
            let item = proc_macro2::TokenStream::from(item);
            let output = gateway_macro::connection_hook_attr::handle_conn_hook($hook, item);
            proc_macro::TokenStream::from(output.unwrap_or_else(|e| e.to_compile_error()))
        }
    };
}

ws_connection_hook_macro!(
    on_connect,
    gateway_macro::connection_hook_attr::ConnHook::OnConnect,
    "Connection hook on a `#[websocket_gateway]` struct: `async fn(&self, client: &WsClient) -> Result<(), WsError>`, run when a client connects. Returning `Err` rejects the connection. Needs no `#[subscriptions]`."
);
ws_connection_hook_macro!(
    on_disconnect,
    gateway_macro::connection_hook_attr::ConnHook::OnDisconnect,
    "Connection hook: `async fn(&self, client: &WsClient)`, run when a client disconnects."
);
ws_connection_hook_macro!(
    after_init,
    gateway_macro::connection_hook_attr::ConnHook::AfterInit,
    "Connection hook: `async fn(&self)`, run once after the gateway path is registered, before any connections."
);

/// Marks a method as a WebSocket message handler for a specific event.
///
/// Similar to `#[get]`, `#[post]` for HTTP routes but for WebSocket events.
///
/// # Syntax
///
/// ```rust,ignore
/// #[subscribe_message("event_name")]
/// async fn handler(&self, client: WsClient, message: WsMessage) -> Result<Option<WsMessage>, WsError>
/// ```
///
/// # Examples
///
/// ```rust,ignore
/// #[subscribe_message("ping")]
/// async fn handle_ping(&self, client: WsClient, message: WsMessage) -> Result<Option<WsMessage>, WsError> {
///     Ok(Some(WsMessage::text("pong")))
/// }
/// ```
#[proc_macro_attribute]
pub fn subscribe_message(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}

// ============================================================================
// RPC CONTROLLER MACROS
// ============================================================================

/// RPC controller macro for defining message pattern handlers.
///
/// Similar to `#[websocket_gateway]` but for RPC transports (TCP, NATS, Kafka, etc.).
/// Implements `RpcControllerTrait` and routes incoming messages by pattern.
///
/// # Syntax
///
/// ```rust,ignore
/// #[rpc_controller(pub struct OrdersController { ... })]
/// impl OrdersController {
///     #[message_pattern("order.create")]
///     async fn create_order(&self, data: RpcData, ctx: RpcContext) -> Result<RpcData, RpcError> { ... }
///
///     #[event_pattern("order.cancelled")]
///     async fn on_order_cancelled(&self, data: RpcData, ctx: RpcContext) -> Result<(), RpcError> { ... }
/// }
/// ```
#[proc_macro_attribute]
pub fn rpc_controller(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attr = proc_macro2::TokenStream::from(attr);
    let item = proc_macro2::TokenStream::from(item);
    let output = rpc_macro::rpc_impl::handle_rpc_controller(attr, item);
    proc_macro::TokenStream::from(output.unwrap_or_else(|e| e.to_compile_error()))
}

/// Marks a method as a request-response RPC handler for a specific pattern.
///
/// The handler receives an `RpcData` payload and returns `Result<RpcData, RpcError>`.
/// The framework sends the returned data back to the caller.
///
/// # Syntax
///
/// ```rust,ignore
/// #[message_pattern("pattern.name")]
/// async fn handler(&self, data: RpcData, ctx: RpcContext) -> Result<RpcData, RpcError>
/// ```
#[proc_macro_attribute]
pub fn message_pattern(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}

/// Marks a method as a fire-and-forget RPC event handler for a specific pattern.
///
/// The handler receives an `RpcData` payload and returns `Result<(), RpcError>`.
/// No response is sent back to the caller.
///
/// # Syntax
///
/// ```rust,ignore
/// #[event_pattern("pattern.name")]
/// async fn handler(&self, data: RpcData, ctx: RpcContext) -> Result<(), RpcError>
/// ```
#[proc_macro_attribute]
pub fn event_pattern(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}

// ============================================================================
// gRPC SERVICE MACROS
// ============================================================================

/// Declares a struct as a gRPC service that the framework discovers and
/// registers with the gRPC adapter at bind time.
///
/// Lives on the struct declaration plus its inherent impl block (parallel
/// to `#[rpc_controller]`). The proto trait impl gets `#[grpc_methods]`
/// separately.
///
/// # Example
///
/// ```rust,ignore
/// #[grpc_service(pub struct OrdersGrpcService { #[inject] repo: OrdersRepo })]
/// impl OrdersGrpcService {
///     pub fn new(repo: ::std::sync::Arc<OrdersRepo>) -> Self { Self { repo } }
/// }
///
/// #[grpc_methods]
/// impl orders_proto::orders_server::Orders for OrdersGrpcService { /* … */ }
/// ```
#[proc_macro_attribute]
pub fn grpc_service(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attr = proc_macro2::TokenStream::from(attr);
    let item = proc_macro2::TokenStream::from(item);
    let output = grpc_macro::grpc_service::handle_grpc_service(attr, item);
    proc_macro::TokenStream::from(output.unwrap_or_else(|e| e.to_compile_error()))
}

/// Annotates `impl SomeProtoTrait for YourService` with the wiring that
/// makes the service register itself with the gRPC adapter.
///
/// The wrapping `*Server` type is inferred from the proto trait's name
/// (`OrdersService` → `OrdersServer` in the same parent path). Override
/// when needed:
///
/// ```rust,ignore
/// #[grpc_methods(server = orders_proto::OrdersServer)]
/// impl orders_proto::orders_server::Orders for OrdersGrpcService { /* … */ }
/// ```
///
/// The annotated impl block is passed through unchanged; only an
/// additional `impl GrpcServiceTrait for YourService` block is emitted
/// alongside it.
#[proc_macro_attribute]
pub fn grpc_methods(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attr = proc_macro2::TokenStream::from(attr);
    let item = proc_macro2::TokenStream::from(item);
    let output = grpc_macro::grpc_methods::handle_grpc_methods(attr, item);
    proc_macro::TokenStream::from(output.unwrap_or_else(|e| e.to_compile_error()))
}
