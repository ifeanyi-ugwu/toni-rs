// Tests: what these macros expand to is proved by `integration-tests`, which
// compiles and runs the generated code against a real application. What they
// refuse to expand is proved by `tests/diagnostics.rs` — a trybuild case per
// documented error message, plus a guard asserting each case still fails for
// the reason it was written for. The unit tests under `src/` cover the parsing
// that fails before any expansion exists to run.

extern crate proc_macro2;

use proc_macro::TokenStream;

mod app_error_macro;
mod catch_macro;
mod config_macro;
mod controller_macro;
mod enhancer;
mod gateway_macro;
mod grpc_macro;
mod key_macro;
mod markers_params;
mod middleware_macro;
mod module_macro;
mod provider_macro;
mod provider_variants;
mod rpc_macro;
mod shared;
mod utils;

/// Declares a module: what it imports, provides, serves and exports. Place it on a unit struct, or
/// on an `impl` block holding its lifecycle hooks and `configure_middleware`, where the macro
/// declares the struct itself.
///
/// ```ignore
/// #[module(
///     imports: [ConfigModule::<AppConfig>::new(), DbModule::for_root(DATABASE_URL)],
///     providers: [UserService, provide!(Port => 3000u16)],
///     controllers: [UserController, api::AdminController],
///     exports: [UserService, Port],
///     global: true,
/// )]
/// pub struct UserModule;
/// ```
///
/// | List | Takes |
/// | --- | --- |
/// | `imports:` | modules, as values: a `#[module]` type named bare, or any expression building a module, such as `DbModule::for_root(url)` or `ConfigModule::<AppConfig>::new()` |
/// | `providers:` | declarations, as values: a bare path is the type's own declaration, `Db` or `repo::Db`, a generic type taking a turbofish, `Extension::<User>`; any other expression, such as `provide!(..)` or `Db::provide()`, is used as it is |
/// | `controllers:` | dispatch targets, as values: a bare path is a `#[controller]` type's own dispatch target, qualified or not; any other expression is a controller factory |
/// | `exports:` | key types: a type, a marker, a generic type with or without a turbofish, `dyn Trait` |
///
/// `global: true` makes the module's exports visible to every module without an import.
///
/// `DynamicModule::builder(..)` takes the same values, a bare path written out as `Db::provide()` or
/// `<Orders as DeclaresController>::controller_factory()`: `.provider(value)`, `.controller(value)`,
/// `.export::<K>()` and `.global()`. An export names a slot and needs no `Key`, so a foreign type is
/// exported by its own type. A string in `exports:` is refused: a key is a type.
#[proc_macro_attribute]
pub fn module(attr: TokenStream, item: TokenStream) -> TokenStream {
    module_macro::module_struct::module(attr, item)
}

/// Every handler macro (`#[routes]`, `#[patterns]`, `#[subscriptions]`, `#[grpc_methods]`)
/// consumes and strips enhancer attributes, so an enhancer macro that reaches expansion is
/// misplaced — most often written above the handler macro, where it expands first and is
/// gone before the handler macro runs. The item is re-emitted alongside the error to keep
/// downstream name resolution intact.
fn unconsumed_enhancer_error(name: &str, item: TokenStream) -> TokenStream {
    let message = format!(
        "#[{name}] was not consumed by a handler macro. Place it below #[routes], #[patterns], \
         #[subscriptions], or #[grpc_methods] on the handler impl, or on a handler method inside \
         it — attributes above the handler macro expand first and never reach it."
    );
    let error = syn::Error::new(proc_macro2::Span::call_site(), message).to_compile_error();
    let item = proc_macro2::TokenStream::from(item);
    quote::quote! { #error #item }.into()
}

/// Field-injection provider — the way to declare a DI provider. Place it on the struct:
/// `#[inject]` fields are dependencies, `#[default(expr)]` fields are owned state. The macro adds
/// the `Clone` impl the container needs, so the struct carries no derive ceremony.
///
/// `scope = "execution"` or `"transient"` overrides the default, singleton.
///
/// Construction logic (`#[new]`) and lifecycle hooks (`#[on_module_init]`, …) live on the struct's `impl`.
///
/// ```ignore
/// #[injectable(scope = "execution")]
/// pub struct UserService {
///     #[inject] repo: UserRepo,
/// }
/// ```
///
/// # `#[inject]`
///
/// | Written | Reads |
/// | --- | --- |
/// | `#[inject] db: Db` | the slot keyed by the field's type |
/// | `#[inject] logger: Arc<dyn Logger>` | the slot keyed by `dyn Logger` |
/// | `#[inject] plugins: Vec<Arc<dyn Plugin>>` | the collection keyed by `dyn Plugin` |
/// | `#[inject(Replica)] db: Db` | the slot under the marker `Replica`, declared `key!(pub Replica: Db)` |
/// | `#[inject(keys::Audit)] log: Arc<dyn Logger>` | a marker holding `dyn Logger`, qualified or not |
/// | `#[inject(Legacy)] legacy: Vec<Arc<dyn Plugin>>` | the collection under a marker holding `dyn Plugin` |
/// | `#[inject(dyn Logger)] logger: Arc<dyn Logger>` | a trait object's own slot, the key written out |
/// | `#[inject(Repo<User>)] repo: Repo<User>` | a type as its own key, generic or not, when it implements `Key` |
///
/// The key is checked against what the field holds, at the key: `V` for a slot holding a sized
/// type, `Arc<dyn Trait>` for one holding a trait object, `Vec<Arc<dyn Trait>>` for a collection.
/// A field holding another type fails to compile there, as do a key that is not a `Key` and a string
/// key. A `#[new]` parameter takes the same keys and cannot take a collection.
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
///     #[get("/")] async fn list(&self) -> impl IntoOutput<Http> { /* … */ }
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
/// Declares a Server-Sent Events handler. Always routes as GET.
///
/// The handler returns a stream of events, not a response type directly:
///
/// ```rust,ignore
/// // Infallible — each event always produces a value
/// #[sse("/events")]
/// async fn events(&self) -> impl Stream<Item = SseEvent> { ... }
///
/// // Per-event fallible — individual events may fail
/// #[sse("/events")]
/// async fn events(&self) -> impl Stream<Item = Result<SseEvent, MyError>> { ... }
///
/// // Setup that can fail before streaming starts — validating a subscription token, say
/// #[sse("/events")]
/// async fn events(&self) -> Result<impl Stream<Item = SseEvent>, MyError> { ... }
/// ```
///
/// The stream outlives the call, so it may not borrow from the handler — `impl Stream<..>` in
/// these signatures captures nothing, which is what lets them be written as shown. A stream
/// that does borrow fails on the handler's own signature.
///
/// Any stream type works — boxed, aliased, or written as `impl Stream`. An error from the third
/// form reaches the error chain like any other handler's: it is raised before the answer is a
/// stream. An error from a *per-event* stream is not, and ends the transfer instead, which a
/// client reconnects after. `SseItem`'s docs say what that costs.
///
/// `#[get]` is the same route written out: return `Sse::new(stream)`, or a `Result` of it. The
/// one difference is what the route declares: only `#[sse]` marks the answer as streamed, so
/// only `#[sse]` is refused at startup on an adapter that collects a response body before
/// sending it. The `#[get]` form registers there and answers whole, or not at all if the
/// stream never ends.
#[proc_macro_attribute]
pub fn sse(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}

/// Applies guards to route handlers or controllers for request authorization.
///
/// Guards execute before the route handler and can block requests based on custom logic.
/// Multiple guards can be specified and execute in the order listed.
///
/// # Syntax
///
/// - **A type** - the binding under it: a type registered in the DI container, a qualified or
///   generic path, or a marker declared with `key!`:
///   ```rust,ignore
///   #[use_guards(AuthGuard)]
///   #[use_guards(Named)] // key!(pub Named: dyn Guard<HttpContext>)
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
/// - **A held value** - a value held in a const is written `value X`, and one held in a `static`
///   `value &X`:
///   ```rust,ignore
///   #[use_guards(value ADMIN)]
///   #[use_guards(value &LIMITER)]
///   ```
///
/// - **Closure** - Builds the guard once per execution, from that execution's context:
///   ```rust,ignore
///   #[use_guards(|ctx| AuditGuard::for_call(ctx))]
///   ```
///
/// The spelling decides the lifecycle: a type resolves from DI with whatever scope its binding
/// declares, a struct literal, constructor call or held value is built once at startup and shared
/// by every execution, and a closure runs per execution. Entries run in the order written,
/// whichever spellings they mix.
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
/// #[routes]
/// #[use_guards(AuthGuard{})]
/// impl MyController {
///     // All methods require authentication
/// }
/// ```
///
/// # Placement
///
/// The attribute is consumed by the handler macro (`#[routes]`, `#[patterns]`,
/// `#[subscriptions]`, `#[grpc_methods]`), so it must sit below it on the handler impl, or on
/// a handler method inside it. Above the handler macro it expands first and never reaches the
/// scan; anywhere it goes unconsumed is a compile error:
///
/// ```compile_fail
/// # use ulo_macros::use_guards;
/// #[use_guards(AuthGuard)]
/// struct NotAHandlerImpl;
/// ```
///
/// # Execution Order
///
/// Guards execute in hierarchical order:
/// 1. Global guards (registered via `UloFactory`)
/// 2. Controller-level guards
/// 3. Method-level guards
///
/// Within each level, guards execute in the order specified.
#[proc_macro_attribute]
pub fn use_guards(_attr: TokenStream, item: TokenStream) -> TokenStream {
    unconsumed_enhancer_error("use_guards", item)
}

/// Applies interceptors to route handlers or controllers for cross-cutting concerns.
///
/// Interceptors wrap request/response handling, allowing you to execute logic before and after
/// the route handler. Common uses include logging, timing, transformation, and caching.
///
/// # Syntax
///
/// - **A type** - the binding under it: a type registered in the DI container, a qualified or
///   generic path, or a marker declared with `key!`:
///   ```rust,ignore
///   #[use_interceptors(LoggingInterceptor)]
///   #[use_interceptors(Named)] // key!(pub Named: dyn Interceptor<HttpContext, HttpHandlerResult>)
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
/// - **A held value** - a value held in a const is written `value X`, and one held in a `static`
///   `value &X`:
///   ```rust,ignore
///   #[use_interceptors(value TIMING)]
///   #[use_interceptors(value &SHARED_TIMING)]
///   ```
///
/// - **Closure** - Builds the interceptor once per execution, from that execution's context:
///   ```rust,ignore
///   #[use_interceptors(|ctx| TimingInterceptor::for_call(ctx))]
///   ```
///
/// The spelling decides the lifecycle, as for [`macro@use_guards`]: a type resolves from DI, a
/// struct literal, constructor call or held value is built once and shared, a closure runs per
/// execution.
/// Entries run in the order written, whichever spellings they mix.
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
/// #[routes]
/// #[use_interceptors(LoggingInterceptor{})]
/// impl MyController {
///     // All methods are logged
/// }
/// ```
///
/// # Placement
///
/// Consumed by the handler macro — see [`macro@use_guards`]: it must sit below `#[routes]` (or
/// its transport counterpart) on the handler impl, or on a handler method inside it; anywhere
/// it goes unconsumed is a compile error.
///
/// # Execution Order
///
/// Interceptors execute in hierarchical order with nested "before" and "after" phases:
/// 1. Global interceptors (registered via `UloFactory`)
/// 2. Controller-level interceptors
/// 3. Method-level interceptors
/// 4. Route handler executes
/// 5. Method-level interceptors (after phase, reverse order)
/// 6. Controller-level interceptors (after phase, reverse order)
/// 7. Global interceptors (after phase, reverse order)
#[proc_macro_attribute]
pub fn use_interceptors(_attr: TokenStream, item: TokenStream) -> TokenStream {
    unconsumed_enhancer_error("use_interceptors", item)
}

/// Applies error handlers to route handlers or controllers for custom error processing.
///
/// Error handlers catch errors from route handlers and return custom HTTP responses.
/// They follow a chain-of-responsibility pattern where specialized handlers can pass
/// errors to more generic handlers by returning None.
///
/// # Syntax
///
/// - **A type** - the binding under it: a type registered in the DI container, a qualified or
///   generic path, or a marker declared with `key!`:
///   ```rust,ignore
///   #[use_error_handlers(CustomErrorHandler)]
///   #[use_error_handlers(Named)] // key!(pub Named: dyn ErrorHandler<HttpContext, HttpHandlerResult>)
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
/// - **A held value** - a value held in a const is written `value X`, and one held in a `static`
///   `value &X`:
///   ```rust,ignore
///   #[use_error_handlers(value FALLBACK)]
///   #[use_error_handlers(value &SHARED_FALLBACK)]
///   ```
///
/// An error handler has no per-execution arm, and `#[use_error_handlers(|ctx| ..)]` is refused
/// where it is written. Entries are consulted last-written first, whichever spellings they mix: a
/// method's entries before its controller's, and both before the globals.
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
/// #[routes]
/// #[use_error_handlers(CustomErrorHandler{})]
/// impl MyController {
///     // All methods use custom error handling
/// }
/// ```
///
/// # Placement
///
/// Consumed by the handler macro — see [`macro@use_guards`]: it must sit below `#[routes]` (or
/// its transport counterpart) on the handler impl, or on a handler method inside it; anywhere
/// it goes unconsumed is a compile error.
///
/// # Execution Order
///
/// Error handlers execute in reverse hierarchical order (most specific first):
/// 1. Method-level error handlers (in order specified)
/// 2. Controller-level error handlers (in order specified)
/// 3. Global error handlers (registered via `UloFactory`)
///
/// Each handler can return Some(response) to handle the error, or None to pass
/// to the next handler. If all handlers return None, a default 500 error is returned.
#[proc_macro_attribute]
pub fn use_error_handlers(_attr: TokenStream, item: TokenStream) -> TokenStream {
    unconsumed_enhancer_error("use_error_handlers", item)
}

/// Attaches metadata to a route handler for use by guards, interceptors, or other enhancers.
///
/// Route metadata is stored once at startup and shared across all requests to the route.
/// Guards and interceptors read it via `context.metadata()`, which returns
/// `Option<&Metadata>` — `None` for global handlers (404, error filters) that never
/// bind to a specific route.
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
/// fn admin_panel(&self) -> Body { ... }
///
/// // Read in guard
/// #[async_trait]
/// impl Guard<HttpContext> for RolesGuard {
///     async fn can_activate(&self, context: &HttpContext) -> bool {
///         if let Some(Roles(required)) = context.metadata().and_then(|m| m.get::<Roles>()) {
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
/// fn get_data(&self) -> Body { ... }
/// ```
///
/// # Two levels
///
/// The attribute applies to the handler impl block as well as to a handler, and the handler wins
/// where both declare the same type. Everything else the block declares is still there:
///
/// ```rust,ignore
/// #[routes]
/// #[set_metadata(Tier("standard"))]
/// #[set_metadata(Audience("internal"))]
/// impl Reports {
///     // reads Tier("standard") and Audience("internal")
///     #[get("/summary")]
///     fn summary(&self) -> Body { ... }
///
///     // reads Tier("premium") and Audience("internal")
///     #[get("/full")]
///     #[set_metadata(Tier("premium"))]
///     fn full(&self) -> Body { ... }
/// }
/// ```
///
/// # Transports
///
/// Works the same on `#[routes]`, `#[subscriptions]`, `#[patterns]` and `#[grpc_methods]`. Read it
/// back with `ctx.metadata()`, which every context carries, so a guard written over
/// `ExecutionContext` reads it on any of them.
///
/// On gRPC a handler reaches the context differently rather than not at all. The tonic trait
/// dictates that signature, so guards, interceptors and error handlers receive one as a parameter
/// and a handler takes it off the request — `GrpcContext::of(request.extensions())`. What the
/// service declared is readable either way. Everywhere else a handler takes the context or an
/// extractor over it as an ordinary parameter.
///
/// A type nothing declared reads back as absent rather than as an error, which is what lets one
/// guard serve annotated and unannotated handlers alike.
///
/// # Replacing or accumulating
///
/// Both declarations are kept, and the reader picks which it wants:
///
/// ```rust,ignore
/// metadata.get::<Roles>()      // the handler's, or the block's where the handler declared none
/// metadata.get_all::<Roles>()  // both, block first
/// ```
///
/// `get` is the common case — most metadata is a setting, and the nearer declaration is the one
/// that applies. `get_all` is for declarations that add up, where a handler's roles extend its
/// controller's rather than replacing them. Nothing is combined for you: what it means to combine
/// two values is known where their type is defined.
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
/// from the DI container (by type, or through a key with `#[inject(K)]`) and passed in — so a
/// dependency can be a constructor argument without being a stored field, and the constructor can
/// run real assembly logic. Without `#[new]`, the provider builds the struct by field injection
/// instead.
///
/// A type has one `#[new]` constructor, and a second fails to compile. A `#[default]` field beside
/// it also fails to compile: the constructor sets every field, so the default could never apply.
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
    "Lifecycle hook on a `#[injectable]` struct: `async fn(&self) -> ulo::di::InitResult`, run after the DI container is built. Returning `Err` aborts startup. Refused on an execution-scoped or transient `#[injectable]`, which the framework holds no instance of to run it on. On a dispatch target built per call, it runs on each instance, and an `Err` fails that call as `ulo::errors::HookFailed`."
);
lifecycle_hook_macro!(
    on_application_bootstrap,
    provider_macro::lifecycle_attr::Hook::OnBootstrap,
    "Lifecycle hook: `async fn(&self) -> ulo::di::InitResult`, run after all modules initialize, before the server accepts connections. Refused on an execution-scoped or transient `#[injectable]`, which the framework holds no instance of to run it on. On a dispatch target built per call, it runs on each instance after `#[on_module_init]`, and an `Err` fails that call as `ulo::errors::HookFailed`."
);
lifecycle_hook_macro!(
    on_module_destroy,
    provider_macro::lifecycle_attr::Hook::OnDestroy,
    "Lifecycle hook: `async fn(&self)`, run as the module is torn down during shutdown. Refused on an execution-scoped or transient `#[injectable]`, which the framework holds no instance of to run it on. Not run on a dispatch target built per call, which leaves the framework no instance at shutdown."
);
lifecycle_hook_macro!(
    before_application_shutdown,
    provider_macro::lifecycle_attr::Hook::BeforeShutdown,
    "Lifecycle hook: `async fn(&self, signal: Option<String>)`, run before shutdown begins. Refused on an execution-scoped or transient `#[injectable]`, which the framework holds no instance of to run it on. Not run on a dispatch target built per call, which leaves the framework no instance at shutdown."
);
lifecycle_hook_macro!(
    on_application_shutdown,
    provider_macro::lifecycle_attr::Hook::OnShutdown,
    "Lifecycle hook: `async fn(&self, signal: Option<String>)`, run as the application shuts down. Refused on an execution-scoped or transient `#[injectable]`, which the framework holds no instance of to run it on. Not run on a dispatch target built per call, which leaves the framework no instance at shutdown."
);

#[proc_macro_derive(Config, attributes(env, default, nested))]
pub fn derive_config(input: TokenStream) -> TokenStream {
    config_macro::derive_config(input)
}

/// Makes a type writable in a key position, naming its own slot: `impl Key for T { type Value
/// = T; }`.
///
/// A type is provided and injected by its own type without this. It is needed where a position
/// reads a slot's `Value` through a key, such as `provide!(T => ..)` and the `_key` lookups.
/// `#[injectable]`, `#[controller]` and `#[websocket_gateway]` implement `Key` already.
#[proc_macro_derive(Key)]
pub fn derive_key(input: TokenStream) -> TokenStream {
    let input = proc_macro2::TokenStream::from(input);
    proc_macro::TokenStream::from(
        key_macro::derive_key(input).unwrap_or_else(|e| e.to_compile_error()),
    )
}

/// Derive `ulo::Error` from an annotated error type.
///
/// Tag the type (or each enum variant) with `#[error_kind(KIND)]`, where
/// `KIND` is a variant of `ulo::ErrorKind`. Untagged variants fall back
/// to a top-level `#[error_kind(...)]` if present, otherwise to
/// `ErrorKind::Internal`.
///
/// ```ignore
/// use ulo::Error;
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

/// Declares a provider: `provide!(key => source)`, `provide!(into key => source)` for a
/// contribution to a collection, or `provide!(source)` keyed by what the source builds.
///
/// What each form builds is a value-API value, so it can equally be written with the value API,
/// passed to `DynamicModule::builder().provider(..)` or returned from a function.
///
/// ```ignore
/// key!(pub Port: u16);
/// key!(pub ApiKey: String);
/// key!(pub Auth: dyn Guard<HttpContext>);
/// key!(pub Legacy: dyn Plugin);
///
/// providers: [
///     provide!(Port => 3000u16),                                 // a value under a marker
///     provide!(ApiKey => value key),                             // a value held in a variable
///     provide!(async |cfg: Config| Db::connect(&cfg.url).await), // keyed by what it builds
///     provide!(dyn Logger => ConsoleLogger),                     // a trait bound to an implementation
///     provide!(Auth => HeaderGuard("x-auth")),                   // a guard: the key's `Value` says so
///     provide!(into dyn Guard<HttpContext> => RateLimit(100)),   // a global HTTP guard
///     provide!(into dyn Plugin => async |d: Dep| B(d)),          // a contribution, by element type
///     provide!(into Legacy => A {}),                             // a contribution to a marker's collection
/// ]
/// ```
///
/// # Keys
///
/// | Key | Written | Holds |
/// | --- | --- | --- |
/// | a marker | `Port`, `keys::Auth`, declared `key!(pub Port: u16)` | what its `Value` names; under a role trait object such as `dyn Guard<HttpContext>`, a value implementing it, registered in that role |
/// | a type as its own key | `Db`, `Repo<User>`, a type implementing `Key` | a value of that type |
/// | a trait object | `dyn Logger` | one implementation, handed out as `Arc<dyn Logger>` |
/// | none | `provide!(Db)`, `provide!(3000u16)`, `provide!(async \|..\| ..)` | the source, under the type it builds |
///
/// A generic key is written with or without a turbofish. A slot holding the declared type hands the
/// value out as the declaration would under its own type; a slot holding a trait object hands out
/// `Arc<dyn Trait>`, the macro writing the conversion. A trait object is held behind `Arc` by a
/// provider shared across threads, so the trait needs `Send + Sync` as supertraits, or the key is
/// written `dyn Trait + Send + Sync`, and the declaration and the field must spell it the same way.
///
/// # Sources
///
/// | Source | Written | Is |
/// | --- | --- | --- |
/// | a type | `Db`, `db::Db`, `Repo<User>`, `Repo::<User>` | the type's own `#[injectable]` declaration; under a key, a second instance with the type's roles and hooks |
/// | a value | `3000u16`, `Config { .. }`, `Client::new(url)` | built where the declaration is written |
/// | a held value | `value PORT`, `value key` | a value in a const or a variable |
/// | a factory | `async \|cfg: Config\| ..`, `\|\| async { .. }` | built at startup; each parameter a dependency resolved by its type |
/// | a held factory | `factory make_pool` | a factory function held in a path |
/// | an alias | `alias Port`, `alias dyn Logger`, `alias Db` | a second name for the binding under that type: the same binding, in its scope |
///
/// A keyword reads as one only where the source does not read as a single expression, so
/// `value.clone()` or `value - 1` keeps `value` as a variable; `value &X` and `factory` before a
/// closure's bars are keywords all the same.
///
/// # Scopes
///
/// A factory is a singleton. `.per_execution()` builds it once per execution and `.transient()` at
/// every resolution, chained after the macro, for a factory under its own type, under a marker
/// holding its type, or under a slot holding a guard or an interceptor, where the binding fills the
/// role's per-execution arm. A factory is always async: a sync closure is refused.
///
/// # Collections
///
/// | Written | Contributes to | Injected as |
/// | --- | --- | --- |
/// | `provide!(into dyn Plugin => A {})` | the collection keyed by `dyn Plugin` | `#[inject] plugins: Vec<Arc<dyn Plugin>>` |
/// | `provide!(into Legacy => A {})` | the collection under `key!(pub Legacy: dyn Plugin)` | `#[inject(Legacy)] legacy: Vec<Arc<dyn Plugin>>` |
/// | `provide!(into dyn Guard<HttpContext> => RateLimit(100))` | HTTP's global guards | — |
///
/// Every source but `alias` contributes. A contribution from a type builds its own instance, apart
/// from the type's plain registration, and takes the collection's role rather than its own. The
/// unnamed collections of `dyn Guard<HttpContext>` and `dyn Interceptor<HttpContext, Answer<Http>>`
/// are HTTP's global guards and interceptors.
///
/// # Refused
///
/// - a string key: a key is a type;
/// - a key that is not a `Key`, and a value its `Value` cannot hold;
/// - `alias` with no key, or after `into`, and `into` with no key;
/// - a sync factory;
/// - a bare path naming a type with no declaration of its own;
/// - at startup, a contribution to any other role collection (a guard or interceptor of RPC,
///   WebSocket or gRPC, an error handler, middleware), naming the `UloFactory::use_global_*` method
///   that registers it;
/// - at startup, a second binding under one key in one module, naming both entries, and a key bound
///   in any module and collected with `into` in any module, naming both modules;
/// - `.per_execution()` or `.transient()` under a slot holding an error handler, middleware or any
///   trait object other than a guard or an interceptor.
#[proc_macro]
pub fn provide(input: TokenStream) -> TokenStream {
    let input = proc_macro2::TokenStream::from(input);
    let output = provider_variants::provide_expr::handle_provide_expr(input);
    proc_macro::TokenStream::from(output.unwrap_or_else(|e| e.to_compile_error()))
}

/// `#[catch(T)]` — escape hatch for runtime-selected error handling.
///
/// The framework's primary error path: a domain error type implements
/// `ulo::Error` and the active transport renders it. `#[catch]` is for
/// cases that path doesn't reach — re-shaping framework events
/// (`GuardRejection`, etc.) per route or per controller, where one handler
/// claims an error and the chain falls through otherwise.
///
/// Lowers a free `async fn` into a unit struct whose `ErrorHandler<C, R>`
/// impl runs `error.downcast_ref::<T>()` and returns `None` on no match
/// (so the chain advances to the next handler).
///
/// ```ignore
/// use ulo::http::{HttpContext, HttpError, HttpHandlerResult};
///
/// #[catch(HttpError)]
/// async fn render_4xx(err: &HttpError, _ctx: &HttpContext) -> HttpHandlerResult {
///     // custom envelope for HttpError 4xx/5xx in this scope
///     Ok(err.to_response())
/// }
///
/// // Register on a controller / method. A bare name is read as a type to resolve from the
/// // container, where `#[catch]` declares nothing, so the handler is passed as a value:
/// #[use_error_handlers(value render_4xx)]
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
/// Similar to `#[controller]` but for WebSocket connections. Implements `Gateway`
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

/// Pattern handlers for a controller — what makes a `#[controller]` struct dispatch RPC.
///
/// Place it on the struct's impl; the `#[message_pattern]` and `#[event_pattern]` methods are
/// scanned into the pattern router.
///
/// # Syntax
///
/// ```rust,ignore
/// #[controller]
/// pub struct OrdersController {}
///
/// #[patterns]
/// impl OrdersController {
///     #[message_pattern("order.create")]
///     async fn create_order(&self, data: RpcData, ctx: &RpcContext) -> Result<RpcData, RpcError> { ... }
///
///     #[event_pattern("order.cancelled")]
///     async fn on_order_cancelled(&self, data: RpcData, ctx: &RpcContext) -> Result<(), RpcError> { ... }
/// }
/// ```
///
/// The struct goes in its module's `controllers:` list, beside HTTP controllers. A controller is
/// reached by pattern and cannot be injected: what a holder would get is decided by the
/// controller's own dependencies. Put shared behaviour in a provider and inject that.
#[proc_macro_attribute]
pub fn patterns(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let item = proc_macro2::TokenStream::from(item);
    let output = rpc_macro::patterns_attr::handle_patterns(item);
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

/// Serves a proto service from an inherent impl of handlers, and makes the
/// `#[controller]` struct holding them dispatch gRPC.
///
/// The attribute names the proto trait. Each handler is marked `#[grpc_method]`,
/// or `#[grpc_stream]` where the reply is a stream; anything unmarked — the
/// constructor, lifecycle hooks, helpers — stays as written.
///
/// ```rust,ignore
/// #[grpc_methods(orders_proto::orders_server::Orders)]
/// impl OrdersGrpcService {
///     #[new]
///     pub fn new() -> Self { Self {} }
///
///     #[grpc_method]
///     async fn create(&self, Payload(req): Payload<CreateOrderRequest>)
///         -> Result<CreateOrderResponse, OrderError>
///     { /* … */ }
///
///     #[grpc_stream]
///     async fn watch(&self, Payload(req): Payload<WatchRequest>)
///         -> Result<impl Stream<Item = Result<Event, OrderError>> + Send + 'static, OrderError>
///     { /* … */ }
/// }
/// ```
///
/// Every parameter is a `FromContext<GrpcContext>`, in any order: `Payload<T>`
/// for the message, `Inbound<T>` for the caller's stream, `ulo_grpc::GrpcRequest<T>`
/// for the whole request as tonic decoded it, `Extensions`, a custom extractor.
/// `&GrpcContext` passes through. The request is taken once, so two of the
/// first three in one handler fail to compile naming both.
///
/// What each method carries is read off the proto, not the handler: `build.rs`
/// runs `ulo_build::compile_protos("proto/orders.proto")`, which after tonic's
/// codegen writes a `{service}_ulo` module beside the `{service}_server` one,
/// and this macro projects through it. A companion written elsewhere is named
/// on the attribute: `#[grpc_methods(pb::orders_server::Orders, shapes = pb::my_shapes)]`.
///
/// The request is installed on the execution before the guards run, so a guard
/// or interceptor reads a copy with `ctx.message::<T>()` and the handler still
/// takes the original.
///
/// A handler answers with the reply message, or with `tonic::Response<T>` to set
/// reply metadata itself. Its error implements `ulo::Error`, so `#[catch]`
/// matches it here as on every other transport.
///
/// `#[grpc_stream]` reads the trait's associated type from the method name —
/// `watch` pairs with `WatchStream` — which is the pairing tonic-build derives
/// from one proto identifier. A trait that names them independently says so:
/// `#[grpc_stream(StreamProgressStream)]`.
///
/// The wrapping `*Server` type is inferred from the proto trait's name
/// (`OrdersService` → `OrdersServer` in the same parent path). Override when
/// needed:
///
/// ```rust,ignore
/// #[grpc_methods(orders_proto::orders_server::Orders, server = orders_proto::OrdersServer)]
/// ```
#[proc_macro_attribute]
pub fn grpc_methods(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attr = proc_macro2::TokenStream::from(attr);
    let item = proc_macro2::TokenStream::from(item);
    let output = grpc_macro::grpc_methods::handle_grpc_methods(attr, item);
    proc_macro::TokenStream::from(output.unwrap_or_else(|e| e.to_compile_error()))
}
