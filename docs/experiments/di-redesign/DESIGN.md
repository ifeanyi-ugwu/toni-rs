# Dependency injection: end-to-end design

`fw` is a placeholder for the framework's crate prefix. Numbers in brackets like [12] refer to the capability list in the brief.

---

## 0. Principles

1. **One runtime container, validated as a whole graph before anything exists.** Every binding declares its sites statically, so the complete dependency graph is known before a single instance is built. All wiring errors are reported in one pass [44]. Anything that is local to one type (site shapes, hooks, role bounds) is checked by the compiler instead [45].
2. **Macros are sugar.** Every macro expands to calls on the value-level API in §9, which integration crates call directly.
3. **Naming.** Types are nouns (`Dep`, `Many`, `Ext`, `ModuleRef`, `Execution`). Traits are capabilities (`Construct`, `Site`, `Module`, `Guard`).
4. **The core has no runtime.** It uses `std::future`, a `BoxFuture` alias, and a pluggable `Timer` trait. Transports and runtime adapters (such as `fw-tokio`) bring the executor, sockets, timers and signals.
5. **Identity is by type.** Keys are `TypeId`s, so a type alias or a renamed import reads the same binding [9][16].

---

## 1. Crate layout

| Crate | Contents |
|---|---|
| `fw-core` | Keys, bindings, sites, scopes, executions, modules, lifecycle, errors, and enhancer traits generic over a transport |
| `fw-macros` | `#[injectable]`, `#[construct]`, `#[module]`, `#[routes]` |
| `fw-http`, `fw-ws`, `fw-rpc`, `fw-grpc` | Transport marker types, contexts, handler attributes, middleware, servers |
| `fw-tokio` (and others) | `Timer` implementation, OS signal future, spawn helpers |
| Integrations (`fw-sqlx`, `fw-redis`, `fw-config`, ...) | Ordinary modules written against the value API |

---

## 2. The model at a glance

- **Key** is the pair of a type and a qualifier: `Key::of::<PgPool, Replica>()`. The qualifier defaults to `()`.
- **Binding** holds a key, a kind (single or contribution), a declared scope, a recipe (construct, value, factory, alias), its declared sites, its origin module, and its source location (captured with `#[track_caller]`).
- **Site** is a type that describes the key(s) it reads and knows how to read itself: `Dep<T, Q>`, `Many<T, Q>`, `Ext<T>`, `Option<S>`, `ModuleRef`, `ExecutionRef`, plus any site type an integration crate defines.
- **Module** is a Rust type with an identity. It declares imports, bindings, controllers and exports.
- **Graph** is the frozen, validated result of registering every module.
- **App** is the graph plus the singleton store. It moves through typestates: `Wired` → `Connected` → `Bound` → serving → closed. From `Connected` on it hands out an `AppHandle`, a `Clone + Send + Sync` view of the shared state that outlives `serve`.
- **Execution** is one call. It holds a per-execution cache, an extension bag, execution inputs (the request or call context), a cancellation signal, and an optional deadline. The framework or the transport holds it; everything else holds a cheap-clone handle to it. Cancellation is fired by the transport while handles are alive. The execution ends when the last handle drops.

The lifecycle runs like this:

```
register modules (sync)
   → wire: build graph + validate everything      [no instances, no I/O]
   → connect: singletons in dependency order,
              readiness checks, init hooks, bootstrap hooks
   → bind: transports bind sockets
   → serve: each call opens an Execution
   → close(signal): destroy → before-shutdown → sockets closed → shutdown
```

---

## 3. Core types and traits

### 3.1 Keys and qualifiers

```rust
#[derive(Clone, Copy)]
pub struct Key {
    ty: TypeId,
    qualifier: TypeId,
    ty_name: &'static str,   // type_name::<T>(), for diagnostics only
    q_name: &'static str,    // type_name::<Q>(), for diagnostics only
}
// PartialEq, Eq and Hash are written by hand over `ty` and `qualifier`: two keys
// compare on the TypeIds alone. `type_name` is not a const fn, so the names are
// read at `of` and carried as plain `&'static str` fields.

impl Key {
    pub fn of<T: ?Sized + 'static, Q: 'static>() -> Key;
}
// Display: "PgPool", "PgPool @ Replica", "dyn Plugin (collection)"
```

A qualifier is any `'static` type, usually a unit struct: `pub struct Replica;`. Because keys are types, there are no strings to misspell [9]. A `Key` is also a runtime value: an integration crate that holds one can look it up erased through `Resolver::by_key::<T>(key)`, which is the one lookup that can ask for a `T` the key does not hold (§10.2).

### 3.2 Sites

```rust
pub struct Dep<T: ?Sized, Q = ()> {
    inner: Arc<T>,
    _q: PhantomData<fn() -> Q>,
}
// Deref<Target = T>, Clone, Dep::from_arc, Dep::into_arc, Dep::arc

pub struct Many<T: ?Sized, Q = ()> {
    items: Arc<[Arc<T>]>,
    _q: PhantomData<fn() -> Q>,
}
// Deref<Target = [Arc<T>]>, iter()

pub struct Ext<T>(Arc<T>);   // a typed view of per-execution data [12]
pub struct ModuleRef { /* graph + module id + optional execution */ }
pub struct ExecutionRef { /* a cheap-clone handle to the current execution (§3.8) */ }
```

The `Site` trait is how every injection point is read. Fields, constructor parameters, factory parameters and closure parameters all go through it, which is what gives them one rule [11].

```rust
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not an injection site",
    label = "the container cannot provide this",
    note = "write `Dep<{Self}>` to share the container's instance",
    note = "or set it inside the #[construct] fn / mark the field #[injectable(default)]"
)]
pub trait Site: Sized + Send + 'static {
    /// Static description used by the wiring pass: key, kind, optional, needs-execution.
    fn describe(d: &mut SiteDesc);

    /// Read the site from the container.
    fn read(r: &Resolver<'_>) -> impl Future<Output = Result<Self, ResolveError>> + Send;
}
```

Implementations in the core:

| Site | Reads | Notes |
|---|---|---|
| `Dep<T, Q>` | single binding `T @ Q` | the shared `Arc` |
| `Many<T, Q>` | every contribution to `T @ Q` | collection order; eager |
| `Option<S>` | `S`, or `None` where `S` would fail with `NotFound` | optional deps [13] |
| `Ext<T>` | extension `T` of the current execution | needs an execution |
| `ModuleRef` | handle to the enclosing module [37] | |
| `ExecutionRef` | a handle to the current execution | needs an execution |

**Collection order** is one rule for every reader of a collection: depth-first post-order over imports from the root, imports in the order written, then declaration order inside a module. A module's contributions follow those of everything it imports, and two modules with no import edge between them are ordered by where the walk reaches them.

`Many<T>` is the eager form: reading it constructs every contribution. Transports read a role collection lazily through `Resolver::entries::<T>()`, which yields one handle per contribution in collection order, each with its own `resolve().await`; §7's pipeline is written against it.

`Option<S>` is `None` exactly when `S` would fail with `LookupError::NotFound`: a key no module binds, an extension no guard has written, an input this execution did not seed. Every other error propagates: a construction failure, `ExecutionRequired`, an ambiguous module.

Execution inputs, meaning the request or call context, are ordinary `Dep<T>` sites. The transport declares the key as an *input* (§6.4). Integration crates can add their own `Site` types, for example `fw-ws` defines `Session<T>` for per-connection state.

### 3.3 Scopes

```rust
pub mod scope {
    pub struct Singleton;
    pub struct PerExecution;
    pub struct Transient;
    pub struct Auto;   // what #[injectable] means when no scope is written
}

pub trait Scope: sealed::Sealed + 'static {
    const KIND: ScopeKind;
}

#[diagnostic::on_unimplemented(
    message = "lifecycle hooks are only allowed on singletons",
    label = "`{Self}` scope cannot have hooks",
    note = "remove the scope argument from #[injectable], or move the hook to a singleton"
)]
pub trait HookCapable: Scope {}          // Singleton, Auto

#[diagnostic::on_unimplemented(
    message = "a `{S}` type cannot read `{Self}`",
    label = "this site needs an execution",
    note = "declare the type #[injectable(execution)] or #[injectable(transient)]"
)]
pub trait AllowedIn<S: Scope> {}         // implemented per site type and scope
// `{Self}` is the site type; the type that cannot read it is the one declared `{S}`.
```

What `Auto` means depends on the binding's role:
- For a **provider**, `Auto` behaves exactly like `Singleton`. Depending on execution data is refused [21].
- For a **controller or enhancer**, `Auto` resolves to singleton if nothing below it needs an execution, and to per-execution otherwise [22][24].

Writing `#[injectable(singleton)]` explicitly on a controller opts out of this inference, so a controller declared that way that needs execution data is refused.

### 3.4 Construct

```rust
pub trait Construct: Sized + Send + Sync + 'static {
    type Scope: Scope;

    /// Declared sites, used by the wiring pass. Generated from the fields or ctor params.
    fn sites(s: &mut Sites);

    /// Build the instance. Async and fallible [15].
    fn construct(r: &Resolver<'_>)
        -> impl Future<Output = Result<Self, ConstructError>> + Send;

    /// Lifecycle hooks this type implements. The macro fills this in by probing;
    /// a hand-written impl calls the `Hooks<Self>` methods it implements (§9.1).
    fn hooks(_h: &mut Hooks<Self>) {}
}
```

`Construct` is not generic over the resolver. That parameter only existed to keep a door open for compile-time modules, and we ruled that approach out. `sites()` is now the single source of truth for dependencies.

### 3.5 Lifecycle traits

```rust
pub trait OnModuleInit: Construct<Scope: HookCapable> {
    fn on_module_init(&self) -> impl Future<Output = Result<(), BoxError>> + Send;
}
pub trait OnApplicationBootstrap: Construct<Scope: HookCapable> {
    fn on_application_bootstrap(&self) -> impl Future<Output = Result<(), BoxError>> + Send;
}
pub trait OnModuleDestroy: Construct<Scope: HookCapable> {
    fn on_module_destroy(&self) -> impl Future<Output = ()> + Send;
}
pub trait BeforeApplicationShutdown: Construct<Scope: HookCapable> {
    fn before_application_shutdown(&self, signal: &Signal) -> impl Future<Output = ()> + Send;
}
pub trait OnApplicationShutdown: Construct<Scope: HookCapable> {
    fn on_application_shutdown(&self, signal: &Signal) -> impl Future<Output = ()> + Send;
}
```

The `Construct<Scope: HookCapable>` supertrait makes a hook on an explicitly execution-scoped or transient type a compile error [41]. `#[injectable]` fills in `Construct::hooks` using autoref probing (Self is concrete inside the generated impl), so implementing a hook trait is all a user does. No marker attribute is involved.

### 3.6 Module

```rust
pub trait Module: Send + Sync + 'static {
    /// Identity for deduplication and diagnostics [31][35].
    fn identity(&self) -> ModuleIdentity;

    /// Declare everything. Synchronous and free of I/O.
    fn register(&self, m: &mut ModuleDef<'_>);
}

impl ModuleIdentity {
    pub fn of_type<M: 'static>() -> Self;                                   // unit modules
    pub fn of_value<M: Eq + Hash + Clone + Send + Sync + 'static>(m: &M) -> Self; // configured modules
    pub fn label(self, name: &'static str) -> Self;                          // readable name override
}
```

`register` returns `()` and never returns early. A value that may fail to build goes through `m.try_value(result)`, which records an `Err` on the `ModuleDef` for `wire()` to report beside every other wiring error, under a `WiringErrors` entry naming the module and the key. Stopping at the first failure would hide the rest, which is what [44] rules out.

Identity is the module's `TypeId`, plus its configuration value if it has one, plus a qualifier if it's keyed. Two configurations of one type are two modules. The same configuration imported twice is one module [31].

Diagnostic names are the type name, the key and an optional label, as in `DbModule @ Replica`, or `DbModule #2` when unlabeled. The configuration's `Debug` output is never printed, because it may hold credentials.

### 3.7 Transports and enhancer roles

```rust
/// Implemented by marker types in transport crates: fw_http::Http, fw_rpc::Rpc, ...
pub trait Transport: 'static {
    type Cx: Clone + Send + Sync;   // per-call context: a cheap-clone handle to the execution
    type Reply: Send;
}

pub trait Guard<T: Transport>: Send + Sync + 'static {
    fn can_activate(&self, cx: &T::Cx)
        -> impl Future<Output = Result<bool, BoxError>> + Send;
}
pub trait Interceptor<T: Transport>: Send + Sync + 'static {
    fn intercept(&self, cx: &T::Cx, next: Next<'_, T>)
        -> impl Future<Output = Result<T::Reply, BoxError>> + Send;
}
pub trait ErrorHandler<T: Transport>: Send + Sync + 'static {
    fn handle(&self, err: BoxError, cx: &T::Cx)
        -> impl Future<Output = Result<T::Reply, BoxError>> + Send;
}

// dyn-compatible twins with blanket impls, used as role keys
pub type AnyGuard<T> = dyn ErasedGuard<T>;
pub type AnyInterceptor<T> = dyn ErasedInterceptor<T>;
pub type AnyErrorHandler<T> = dyn ErasedErrorHandler<T>;
```

`Guard<Http>` and `Guard<Rpc>` are different traits, so an HTTP guard and an RPC guard are different roles [25]. A role comes only from a trait implementation.

`Cx` wraps an `ExecutionRef` together with the call's wire data, and every clone is the same call. Everything a guard or interceptor writes goes through a shared reference and interior mutability, which the extension bag already uses. A reply that streams carries a clone of `Cx`, which is how a body, a reply stream or a WebSocket item sequence keeps its per-execution instances and its cancellation signal alive past `intercept`'s return. A request body stream lives on `Cx` behind a take-once slot, not as an execution input: inputs are `Sync` (§6.4) and a body stream is read once.

### 3.8 Execution

```rust
pub struct Execution { /* Arc: cache, extensions, inputs, cancel, deadline, module */ }
pub struct ExecutionRef { /* a clone of that Arc */ }   // Clone + Send + Sync

impl Execution {
    pub fn extensions(&self) -> &Extensions;               // typed bag [23]
    pub fn cancelled(&self) -> Cancelled<'_>;              // future, runtime-free
    pub fn is_cancelled(&self) -> bool;
    pub fn deadline(&self) -> Option<Instant>;
    pub fn handle(&self) -> ExecutionRef;                  // an owned clone, for a spawned subtask
    pub fn seed<T: Send + Sync + 'static>(&self, input: T);
    pub async fn get<T: ?Sized + Send + Sync + 'static>(&self) -> Result<Dep<T>, LookupError>;
}
// ExecutionRef has the same methods.
```

`Execution: Send + Sync`, and `Resolver<'_>` is `Sync` with it. That is what lets `Site::read` and `Construct::construct` hold the resolver across an await and still return `Send` futures; a `!Sync` value anywhere in the execution would make every site's future `!Send`, reported at the trait's `+ Send` rather than at the value. `seed` is bounded `Send + Sync` for the same reason.

Who holds the execution: the transport for a call, `execute` for a standalone one. Everything else holds a clone, `ExecutionRef` directly or inside a `Cx`. Two events are distinct:
- **Cancellation** is fired by the transport: the client disconnects, the deadline passes, or shutdown begins. It fires while clones are alive, so a streaming reply observes it.
- **End of execution** is when the last clone drops. The per-execution cache is released then and execution-scoped instances are dropped: a streaming reply keeps its instances exactly as long as it runs.

`get` resolves against the visibility of the execution's module: the dispatching controller's module inside a transport call, the root module in a standalone execution opened on the app, or the module whose `execute` opened it (§8.2).

The execution cache uses a runtime-agnostic async once-cell (for example from `async-lock`, which is not a runtime). If two sites in one execution resolve the same key concurrently, they get one instance. `Cancelled` is implemented in the core with a waker list. The core stores the deadline, and the transport's runtime enforces it.

---

## 4. Bindings [1–10]

A user's service, in the struct style:

```rust
#[injectable]                                  // Auto → singleton for providers [1]
pub struct UserService {
    repo: Dep<dyn UserRepo>,                   // trait object [4]
    db: Dep<PgPool, Primary>,                  // qualified [5]
    plugins: Many<dyn Plugin>,                 // collection [7]
    audit: Option<Dep<AuditLog>>,              // optional [13]
    #[injectable(default)]
    retries: u32,                              // owned state [14]
}
```

The same service in the constructor style, which is the promoted one:

```rust
pub struct UserService { repo: Dep<dyn UserRepo>, db: Dep<PgPool, Primary>, retries: u32 }

#[injectable]
impl UserService {
    // picked automatically because it's named `new`; #[construct] marks any other name
    pub async fn new(
        repo: Dep<dyn UserRepo>,
        db: Dep<PgPool, Primary>,
        cfg: Dep<AppConfig>,
    ) -> Result<Self, InitError> {                         // async + fallible [15]
        Ok(Self { repo, db, retries: cfg.retries })        // owned state [14]
    }
}
```

The module ties these together:

```rust
pub struct Primary;
pub struct Replica;

#[module(
    imports   = [ConfigModule, DbModule::for_root(env_url("DB_PRIMARY")).keyed::<Primary>()],
    providers = [
        UserService,
        PgUserRepo as dyn UserRepo,                 // bind under a trait [4]
        AppConfig::from_env()?,                     // value [2]
        into dyn Plugin: [MetricsPlugin, TracingPlugin],  // collection contributions [7]
    ],
    exports   = [UserService],
)]
pub struct UsersModule;
```

And here is the same thing written against the value API. This is exactly what the macro expands to:

```rust
impl Module for UsersModule {
    fn identity(&self) -> ModuleIdentity { ModuleIdentity::of_type::<Self>() }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(ConfigModule);
        m.provide::<UserService>();
        m.provide::<PgUserRepo>().also_as::<dyn UserRepo>(|a| a);   // coercion in the closure
        m.try_value(AppConfig::from_env());                          // `expr?` in the macro
        m.contribute::<dyn Plugin>().provide::<MetricsPlugin>(|a| a);
        m.contribute::<dyn Plugin>().provide::<TracingPlugin>(|a| a);
        m.export::<UserService>();
    }
}
```

`expr?` in a `providers` list lowers to `m.try_value(expr)`: `register` never returns early (§3.6), and an `Err` is reported by `wire()` beside the other wiring errors. `expr` without `?` lowers to `m.value(expr)` and binds whatever type `expr` has, a `Result` included.

Async factories and third-party types [3][8]:

```rust
// Parameters are sites. The output type is the key. A `try_` factory's output is a
// Result whose Ok type is the key; its Err is a startup error.
m.try_singleton(|cfg: Dep<DbConfig>| async move {
    PgPool::connect(cfg.url.expose()).await
});

m.try_execution(|pool: Dep<PgPool>| async move { UnitOfWork::begin(&pool).await });
m.transient(|| async { RequestId::new() });
```

Each scope has a plain and a fallible method: `singleton`/`try_singleton`, `execution`/`try_execution`, `transient`/`try_transient`. One method cannot serve both outputs, since blanket impls over `T` and `Result<T, E>` overlap, and a marker parameter only moves the failure to the call as a type-annotation error. The `#[module]` macro keeps one spelling: it writes the concrete call site, where autoref ranking over the closure's own type picks the fallible arm for a `Result` future and the plain arm otherwise.

A `Construct` type whose instance needs a custom build goes through `m.provide_with::<T>(factory)` or `m.try_provide_with::<T>(factory)`. The binding is keyed by `T`, uses `T::Scope`, builds through the factory, and runs `T::hooks`, which a plain factory never does (§9.1).

Aliases [6], where both keys reach the same object:

```rust
m.alias::<PgPool, ReadOnly>().of::<Replica>();             // `PgPool @ ReadOnly` reads the `Replica` binding
m.provide::<RedisCache>().also_as::<dyn Cache>(|a| a);     // second key under a trait
```

An alias keeps the type and changes the qualifier; the new key is in the type arguments and the existing one in `of`. A second key under another type is a coercion, which is `also_as`.

Trait coercion is written as `|a| a`. Stable Rust can't unsize generically, but a closure whose expected type is `fn(Arc<RedisCache>) -> Arc<dyn Cache>` coerces at its return. The macro's `X as dyn T` generates exactly this.

Duplicates [10]: a second single binding for a key in the same module, or a mix of `provide` and `contribute` under one key, is a wiring error naming both source locations.

---

## 5. Injection sites [11–17]

Every site obeys one rule: **a site is its type**. The container reads `T @ Q` for `Dep<T, Q>`, whichever identifier or alias the user wrote [16]. Fields, constructor parameters, factory parameters, enhancer closures, hook closures and readiness closures all accept any `Site` [11].

Reading per-execution data in an execution-scoped service [12][23]:

```rust
#[injectable(execution)]
pub struct AuditContext {
    user: Ext<CurrentUser>,             // written by a guard earlier in this execution
    head: Dep<fw_http::RequestHead>,    // execution input seeded by the transport
    exec: ExecutionRef,                 // cancellation + deadline
}
```

Shared instances [17]: `Dep<T>` is an `Arc<T>`, and every holder of a binding holds the same `Arc`, whether it's aliased, exported or re-exported. Mutation goes through interior mutability, and whatever an init hook does is seen by every holder. Services are never cloned. `Dep` clones the pointer.

Compile-time checks on sites:
- A field or parameter that isn't a `Site` produces the `Site` diagnostic, which tells the user to write `Dep<T>` or set the value in the constructor.
- `Dep<T>` requires `T: Send + Sync`. The error is the auto trait's own ("`(dyn Repo + 'static)` cannot be sent between threads safely"), with no hint: rustc reports the unsatisfied `Send` from the bound's where-clause and drops any wrapper trait's `on_unimplemented` note. The fix, `Send + Sync` as supertraits of `Repo`, is documented rather than diagnosed. The other spelling mistake, a binding under `dyn Repo + Send + Sync` read as `dyn Repo` or the reverse, compiles on both sides and is caught at `wire()` (§10.1).
- `Ext<T>` or `ExecutionRef` in a type explicitly declared `singleton` fails the `AllowedIn` bound. The macro emits one `quote_spanned!` assertion per site so the error lands on that field or parameter.
- A constructor whose future isn't `Send` fails at the generated `Construct` impl, pointing at the function.

---

## 6. Scopes and executions [18–23]

### 6.1 Scope declarations

```rust
#[injectable]               // Auto
#[injectable(singleton)]    // explicit singleton
#[injectable(execution)]    // once per execution, shared inside it
#[injectable(transient)]    // fresh at every site
```

Factories choose their scope with the method they're registered through: `m.singleton(..)`, `m.execution(..)`, `m.transient(..)`. Types that implement `Construct` always use `T::Scope`. Registration can't override it, which is what keeps the compile-time hook check sound.

### 6.2 Scope resolution at wiring time

After the cycle check, the graph computes whether each binding **needs an execution**. A binding needs one if any of its sites is `Ext`, `ExecutionRef`, an execution input, or a dependency that itself needs an execution. Transient and auto bindings pass this need upward. The rules are then:

| Binding | Needs an execution | Result |
|---|---|---|
| Explicit singleton | yes | **refused** [21], with the full path |
| Auto provider | yes | **refused**, with the hint "declare it `#[injectable(execution)]`" |
| Auto controller or enhancer | yes | becomes per-execution, built per call [22] |
| Auto type with hooks that became per-execution | — | **refused** (the startup half of [41]) |
| Transient | yes | allowed, but every consumer must be able to run in an execution |

### 6.3 Opening executions

Transports open one execution per HTTP request, WebSocket message, RPC call or gRPC call. A WebSocket connection is not an execution: per-connection state lives in `fw_ws::Session<T>`, a site type read from the execution input the WebSocket transport seeds [19].

Code opens standalone executions itself:

```rust
let report = app
    .execute(ExecOptions::new().deadline(Instant::now() + Duration::from_secs(30)), async |exec| {
        let job = exec.get::<ReportJob>().await?;
        job.run().await
    })
    .await?;
```

```rust
impl App<Connected> {   // also on AppHandle and ModuleRef
    pub async fn execute<F, R>(&self, opts: ExecOptions, f: F) -> R
    where
        F: AsyncFnOnce(&Execution) -> R;
}
```

The closure borrows the execution, which stays owned by `execute`: `execute` drops it when the future completes, and a subtask that outlives the call takes `exec.handle()`, an owned clone that keeps the execution alive until it drops (§3.8). A plain closure returning an `async move` block cannot borrow its argument into the future, which is why the bound is `AsyncFnOnce` and the closure is written `async |exec|`. `execute` is an inherent `async fn` on every type that carries it, never a trait method: its future is `Send` through auto-trait leakage whenever the caller's closure future is, and a trait would have to write that `Send` bound, which stable Rust cannot state for an `AsyncFnOnce` future.

An execution opened on the app resolves with the root module's visibility; one opened through `app.module::<M>()?.execute(..)` resolves with `M`'s (§8.2).

### 6.4 Execution inputs

A transport declares the context types it seeds, together with the transport that seeds them, so the wiring pass knows both:

```rust
// inside fw-http's own global module
m.input::<RequestHead>().seeded_by::<Http>();
m.input::<ClientAddr>().seeded_by::<Http>();
```

At each call, the transport seeds them with `exec.seed(head)`, bounded `T: Send + Sync + 'static` (§3.8). Inputs are app-wide and belong to transports: a keyed module declaring one is a wiring error.

Wiring walks each handler's reachable execution-scoped bindings and checks every non-optional input against that handler's transport. A per-execution service reading `Dep<RequestHead>` on a path from an RPC controller is a wiring error naming the handler, the service and the input. A service shared across transports reads the input as `Option<Dep<RequestHead>>`, which is `None` where the transport did not seed it.

Standalone executions are the one runtime case, since nothing static says what they seed: one that doesn't seed an input which a site reads gets `LookupError::NotFound { kind: Input }` at runtime, and the error names the key.

---

## 7. Enhancers and roles [24–28]

A guard is any type that implements the role trait:

```rust
#[injectable]                                   // Auto: singleton, or per-execution if needed
pub struct AuthGuard { sessions: Dep<SessionStore> }

impl Guard<Http> for AuthGuard {
    async fn can_activate(&self, cx: &HttpCx) -> Result<bool, BoxError> {
        let Some(user) = self.sessions.lookup(cx.head()).await? else { return Ok(false) };
        cx.extensions().insert(CurrentUser(user));     // read later as Ext<CurrentUser>
        Ok(true)
    }
}
```

Declaring enhancers by type, by value or by closure [26]:

```rust
#[routes]
#[guards(http = AuthGuard)]                              // by type, HTTP handlers only: AuthGuard is Guard<Http>
#[interceptors(TimingInterceptor)]                       // every handler: TimingInterceptor is Interceptor<Http> and <Rpc>
impl UsersController {
    #[fw_http::get("/users/:id")]
    #[guards(value = RateLimit::per_second(100))]        // by value: built once, shared
    #[guards(with = |u: Ext<CurrentUser>| RoleGuard::require(u, Role::Admin))]  // closure: per execution
    async fn get(&self, id: Path<u64>) -> Result<Json<User>, HttpError> { /* ... */ }

    #[fw_rpc::message("users.get")]
    async fn get_rpc(&self, req: Payload<GetUser>) -> Result<User, RpcError> { /* ... */ }
}
```

A controller-level enhancer applies to every handler on the impl, strictly. `#[interceptors(TimingInterceptor)]` expands to `spec.interceptor::<TimingInterceptor>()` once per handler, and each expansion requires the role for that handler's transport: `Interceptor<Http>` for `get`, `Interceptor<Rpc>` for `get_rpc`. A handler whose transport lacks the role is a compile error at the attribute naming the handler: written `#[guards(AuthGuard)]`, the impl above fails on `get_rpc` with a message like "`AuthGuard` is not a guard for `Rpc`, needed by `get_rpc`; implement `Guard<Rpc>`". There is no "apply where the role exists": a guard that guards three of four handlers is the bug [25] exists to prevent. A controller that serves several transports scopes an enhancer with the transport-scoped form, `#[guards(http = AuthGuard)]`, which applies it to that transport's handlers alone and checks only their role.

Global enhancers are collection contributions under a role key [28]:

```rust
m.contribute::<AnyGuard<Http>>().provide::<AuthGuard>(|a| a);     // a global HTTP guard
m.contribute::<AnyInterceptor<Rpc>>().value(Arc::new(Tracing::default()));
```

Stack order is global, then controller, then method [26]. Within the global level, contributions follow collection order (§3.2).

The dispatch pipeline for one call [27] runs like this:

1. The transport opens an `Execution`, seeds the inputs, and builds its `Cx` around a handle to it.
2. It reads the role collection through `Resolver::entries::<AnyGuard<T>>()`, which hands it one unresolved entry per contribution, and walks them in stack order together with the controller's and the method's declarations. For each guard it obtains that guard (a shared value, the singleton, `resolve().await` for a per-execution build, or the closure call), then runs `can_activate`. On a refusal it stops. **Later guards are never built.** `Many<AnyGuard<T>>` would build every guard first, which is why the transport reads `entries` and not `Many`.
3. Only once every guard admits does it build the interceptors, then the controller (per call if inferred), then run the handler inside the interceptor chain.
4. Errors go through the error handlers, method level first, then controller, then global. A refusal is one of them: the pipeline turns `Ok(false)` into the core error `GuardRejected { guard: &'static str }` and routes it like any other, so a handler that downcasts the `BoxError` can reshape the response per route. Unclaimed, the transport renders its forbidden status. A guard that wants another status, 401 rather than 403, returns its own `Err`.
5. The transport drops its `Cx` when the reply is written. A streaming reply holds a clone, and the execution ends with the last one (§3.8).

HTTP middleware is configured per module by route [36]. The core stores typed per-module metadata that `fw-http` reads:

```rust
m.meta::<fw_http::Middleware>()
    .apply::<RequestLogger>()
    .for_routes(["/users/*"])
    .exclude(["/users/health"]);
```

WebSocket gateways are controllers whose handlers carry `fw_ws` attributes. A gateway participates in the container like any other controller [24].

---

## 8. Modules [29–38]

### 8.1 Declaring modules

```rust
#[module(
    imports     = [DbModule::for_root(url)],
    providers   = [UserService],
    controllers = [UsersController],
    exports     = [UserService, reexport PgPool],   // re-export from an import [34]
)]
pub struct UsersModule;

#[module(global, providers = [AppConfig::load()?], exports = [AppConfig])]   // global [30]
pub struct ConfigModule;
```

### 8.2 Visibility

A module can see its own bindings, the exports of its direct imports, and the exports of global modules. That's all. An export is the only way a binding leaves its module [29]. A re-export is allowed only for a key the module can see unambiguously [34].

A lookup that names no module uses the root module's visibility: `app.get`, and `exec.get` inside an execution the app opened, see the root's own bindings, its imports' exports and the globals. The root's table is free of ambiguity once wiring passes, so no lookup variant is needed for it. A service exported only within a subtree is unreachable that way by design; it is reached through `app.module::<M>()?`, and a job that belongs to a subtree runs through that module's `execute`. Inside a transport call, the execution's resolver carries the dispatching controller's module: handlers and their per-execution services see what their module sees.

Collections are the exception: `Many<T>` gathers contributions from every module in the application, because plugins and global enhancers are app-wide by nature [7].

### 8.3 Configured and keyed modules

```rust
pub struct DbModule { url: Secret<String> }   // Eq + Hash → identity by value

impl DbModule {
    pub fn for_root(url: impl Into<Secret<String>>) -> Self { Self { url: url.into() } }
}

// two databases from one integration [32]
imports = [
    DbModule::for_root(primary_url).keyed::<Primary>(),
    DbModule::for_root(replica_url).keyed::<Replica>(),
]
// consumers read Dep<PgPool, Primary> and Dep<PgPool, Replica>
```

`Keyed<Q, M>` is itself a module. Inside `M`, sites stay unqualified (`Dep<PgPool>`). At the export boundary, unqualified exports are requalified as `T @ Q`, re-exports included, since they leave the boundary like any export. Contributions are not exports and keep their key: two keyed databases contribute two `dyn HealthIndicator` entries. Execution inputs are app-wide and belong to transports; a keyed module declaring one is a wiring error.

Its identity includes `Q`, so the two instances are distinct modules, and a bare `DbModule::for_root(url)` beside `DbModule::for_root(url).keyed::<Primary>()` is two modules with two pools and two readiness checks. One pool under two keys is an alias (§4), not a second import.

### 8.4 Runtime-built modules [33]

```rust
pub fn redis_module(cfg: RedisConfig) -> DynamicModule {
    DynamicModule::new::<RedisIntegration>(cfg.clone(), move |m| {
        m.singleton(move || { let cfg = cfg.clone(); async move { RedisPool::connect(&cfg).await } });
        m.export::<RedisPool>();
    })
}
```

The identity is the owner type plus the configuration value, so these modules are still distinct Rust-typed identities.

### 8.5 Module handles [37]

```rust
async fn from_anywhere(app: &App<Connected>) -> Result<(), LookupError> {
    let users = app.module::<UsersModule>()?;                    // AmbiguousModule if several configs
    let replica = app.module_keyed::<DbModule, Replica>()?;
    let pool = replica.get::<PgPool>().await?;                   // limited to what that module sees
    Ok(())
}

#[injectable]
pub struct PluginHost { here: ModuleRef }                        // the enclosing module
```

A `ModuleRef` carries `get` and `execute`, both limited to what its module sees. During `connect`, `get` for a singleton the eager walk has not built yet is refused with `LookupError::NotReady { key }`: building it on demand would break the eager order, and `NotFound` would misdescribe a key that exists. Once `connect` returns, `NotReady` cannot occur.

### 8.6 Lazy modules [38]

```rust
let handle: AppHandle = app.handle();                 // Clone + Send + Sync; taken before serve(self)
let reports: ModuleRef = handle.load(ReportsModule).await?;
```

`AppHandle` is a `Clone + Send + Sync` view of the shared inner state, available from `Connected` on. `get`, `module`, `execute`, `load` and `close` live on it; `serve(self)` consumes only the `Bound` typestate, so a handle taken before `serve` keeps working while the app serves. The graph sits behind a lock that only `load` writes.

A lazily loaded module is wired against the frozen graph, with all of its errors reported in one pass, and then connected through its own readiness checks and init hooks. Loading the same identity twice returns the existing handle.

Some things are refused at load time, because the graph has already been handed out:
- **Controllers and middleware**, because routes are already bound.
- **Contributions to a collection the module does not introduce itself.** A lazy module cannot contribute to any key that a pre-existing binding reads as `Many<T>`, whatever that binding's scope; the rule is static and checked at `load` without asking what has already run.
- **Global exports.**

Shutdown includes lazily loaded modules, in reverse order of loading.

---

## 9. Lifecycle [39–43]

### 9.1 Hooks

For types the container constructs, hooks are trait implementations:

```rust
#[injectable]
impl Cache {
    pub fn new(pool: Dep<PgPool>) -> Self { /* ... */ }
}

impl OnModuleInit for Cache {
    async fn on_module_init(&self) -> Result<(), BoxError> { self.warm().await }
}
impl OnApplicationShutdown for Cache {
    async fn on_application_shutdown(&self, signal: &Signal) { self.flush().await }
}
```

For factory outputs (such as a third-party pool) and for modules, hooks are closures registered on a **singleton** binding handle. The handle is typed, so the hook methods don't exist on execution-scoped or transient handles [41]:

```rust
m.try_singleton(|cfg: Dep<DbConfig>| async move { PgPool::connect(cfg.url.expose()).await })
    .on_destroy(|pool: Dep<PgPool>| async move { pool.close().await });

m.on_init(|users: Dep<UserService>| async move { users.seed_admin().await });   // module hook [39]
```

A factory's output is a plain value, so trait hooks run only for types the container constructs. This rule is documented, and it's the reason closure hooks exist. It holds even when the output type implements `Construct`: `m.singleton(|| async { Cache::custom() })` binds `Cache` by factory, and its `OnModuleInit` impl compiles and never runs, because the graph learns which types are `Construct` only through `provide`. A `Construct` type that needs a custom build is bound with `m.provide_with::<Cache>(|| async { Cache::custom() })`, which runs `Cache::hooks` like `provide` does (§4).

`Hooks<T>` has one method per hook trait, each with a `where` bound, so `h.on_module_init()` compiles only when `T: OnModuleInit`, and likewise `on_application_bootstrap`, `on_module_destroy`, `before_application_shutdown` and `on_application_shutdown`. A hand-written `Construct` impl knows which traits it implemented and calls those methods. `#[injectable]` fills `hooks` with autoref probes over the concrete `Self`; the probe types are macro internals, and `fw::hooks!(h)` is the same probing offered as a macro for a hand-written impl that wants all five checked.

### 9.2 Order [40]

The connect phase walks the singleton graph in a stable topological sort: among the bindings whose dependencies are done, the smallest (module post-order index, declaration index) runs next, the module index being the module's position in collection order (§3.2). For each binding it constructs the instance and runs that binding's readiness check. It then runs all `OnModuleInit` hooks in the same order. A module's own hooks run after the hooks of its providers. Last come all `OnApplicationBootstrap` hooks. Two bindings with no edge between them are ordered by the tie-break, so the order is the same on every run.

Close runs everything in the exact reverse: `OnModuleDestroy`, then `BeforeApplicationShutdown(signal)`, then transports close their sockets, then `OnApplicationShutdown(signal)`.

### 9.3 Readiness checks [42]

```rust
m.singleton(|cfg: Dep<DbConfig>| async move { PgPool::connect_lazy(cfg.url.expose()) })
    .ready(|pool: Dep<PgPool>| async move { pool.ping().await })
    .retries(5)
    .backoff(Duration::from_millis(500))
    .timeout(Duration::from_secs(10));
```

A readiness check runs right after its binding is constructed and before anything that depends on it. Retries and timeouts use the app's `Timer`. If a check needs a timer and none is configured, that's a wiring error.

Before an error message leaves the core, it goes through a redaction pass. The pass replaces every `Secret<_>` registered with the graph and, as a backstop, strips the userinfo from anything shaped like a URL. Registration is explicit per graph, never a process-wide list, which would leak between tests:
- `m.secret(&self.url)` in the value API, for a secret a module holds in its configuration and moves into a factory.
- `#[module]` registers every `Secret<_>` field of a configured module, since it can see the fields.
- `m.value(Secret<_>)` registers the value it binds.

### 9.4 Phases [43]

```rust
#[tokio::main]
async fn main() -> Result<(), fw::StartupError> {
    let app = App::builder(AppModule)
        .timer(fw_tokio::Timer)
        .wire()?;                                   // graph + all wiring errors; no instances, no I/O

    let app = app.connect().await?;                 // outbound connections, readiness, hooks

    let app = app
        .bind(fw_http::Server::new("0.0.0.0:8080"))
        .bind(fw_grpc::Server::new("0.0.0.0:50051"))
        .listen()
        .await?;                                    // sockets

    let handle = app.handle();                      // AppHandle: get, module, execute, load, close
    app.serve(fw_tokio::shutdown_signal()).await?;  // runs until the signal, then closes in order
    Ok(())
}
```

A job, a CLI command or a test stops after `connect()` and uses `get` or `execute`, then calls `close(Signal::new("done"))`. Anything that needs the app while it serves holds an `AppHandle` taken before `serve` (§8.6).

---

## 10. Errors and diagnostics [44–47]

### 10.1 The wiring pass

`wire()` runs these steps and **collects** errors. It never stops at the first one.

1. **Module graph:** deduplicate identities, detect import cycles (printing the path of module names), check that re-exports are visible, and refuse an input declared by a keyed module.
2. **Bindings:** find duplicate singles, single/collection mixes, aliases pointing at nothing, values whose `try_value` recorded an `Err` (naming the module and the key), and overrides that match no binding or more than one (in tests).
3. **Visibility:** resolve every site against its module's visibility table. Report missing keys (with the site, the key and the module) and ambiguous keys (naming every source module). When a missing key's name equals a bound key's name up to a trailing `+ core::marker::Send + core::marker::Sync`, the report names both spellings: `dyn Repo` and `dyn Repo + Send + Sync` are distinct `TypeId`s, and this is the one place the mismatch is visible.
4. **Dependency cycles:** run a DFS over the resolved edges and print the full path, as in `A → B → C → A`, with the module of each step.
5. **Scopes:** run the needs-execution pass from §6.2, then report scope violations with the path that introduces the execution dependency, and hooks on types that became per-execution. Then, for each handler, walk its reachable execution-scoped bindings and report every non-optional input that the handler's transport does not seed (§6.4).
6. **Environment:** check for a timer if checks need one.

Steps that depend on a missing piece skip only the affected edges, so one missing binding doesn't hide unrelated errors.

A sample of the output:

```
error: wiring failed with 4 errors

  × missing dependency `dyn Mailer`
    ├─ needed by UserService (param `mailer`) in UsersModule
    └─ help: import a module that exports `dyn Mailer`, or provide it in UsersModule

  × missing dependency `dyn Repo`
    ├─ needed by ReportService (field `repo`) in ReportsModule
    └─ help: PersistenceModule exports `dyn Repo + Send + Sync`; the site reads `dyn Repo`

  × ambiguous dependency `dyn UserRepo` in AppModule
    ├─ exported by PersistenceModule   (src/persistence.rs:14)
    └─ exported by LegacyRepoModule    (src/legacy.rs:9)

  × scope violation: singleton `ReportService` depends on per-execution data
    └─ ReportService → AuditContext (execution) → Ext<CurrentUser>
       help: declare ReportService #[injectable(execution)], or inject a factory
```

### 10.2 Error types

```rust
pub enum StartupError {
    Wiring(WiringErrors),                                         // everything from wire()
    Construct { key: KeyName, module: ModuleName, source: BoxError },
    Readiness { key: KeyName, attempts: u32, source: Redacted },
    Hook { hook: HookKind, key: KeyName, source: BoxError },
    Bind { transport: &'static str, source: BoxError },
}

#[non_exhaustive]
pub enum LookupError {                                           // [47]
    NotFound { key: KeyName, kind: LookupKind },                  // binding, extension, input
    NotReady { key: KeyName },                                    // a singleton the connect walk has not built yet (§8.5)
    WrongType { key: KeyName, requested: &'static str },          // an erased `Key` asked for a `T` it does not hold
    WrongKind { key: KeyName, expected: BindingKind, found: BindingKind },   // single read as collection, or the reverse
    ExecutionRequired { key: KeyName },
    AmbiguousModule { module: &'static str, candidates: Vec<ModuleName> },
    Construct { key: KeyName, source: BoxError },
}

/// A guard's `Ok(false)`, as the error handlers see it (§7).
pub struct GuardRejected { pub guard: &'static str }
```

`WiringErrors` carries one entry per failure from §10.1, the `Err` a `try_value` recorded among them. `WrongType` is reachable from one surface only, `Resolver::by_key::<T>(key)` (§3.1): a typed site's key fixes its `T`, and an erased key does not.

Neither the core nor the macros panic or exit [45]. Panics inside user constructors, factories and hooks are caught at the poll boundary and turned into `StartupError::Construct` or `StartupError::Hook`. That holds unless the binary is built with `panic = "abort"`, where nothing can be caught.

### 10.3 Compile-time diagnostics [46]

Compile-time diagnostics use two mechanisms:
- **Trait-bound errors** carry `#[diagnostic::on_unimplemented]` messages written as instructions ("write `Dep<PgPool>`", "implement `Guard<Http>`", "remove the scope argument").
- **Macro errors** use `syn::Error` spans on the exact token (two `#[construct]` functions, no constructor and no `new`, a provider in `#[module]` that doesn't implement `Construct` together with the hint "add #[injectable] or bind it with a factory").

The macro emits per-site assertions with `quote_spanned!`, so errors point at the offending field or parameter rather than at the macro invocation.

---

## 11. Testing [48]

```rust
let app = TestApp::of(AppModule)
    .override_value::<dyn UserRepo>(Arc::new(InMemoryRepo::default()))
    .override_factory(|| async { FixedClock::at(t0) })
    .override_try_factory(|| async { TestBus::connect().await })
    .override_value::<AuditLog>(Arc::new(NullAudit)).in_module::<BillingModule>()
    .replace_module(MailModule, FakeMailModule)
    .connect()
    .await?;

let users = app.get::<UserService>().await?;
```

Override rules:
- An override replaces the recipe of an existing key and keeps its origin module, its visibility and its exports, so production modules stay untouched.
- An override that matches no binding is a wiring error, which catches stale mocks after refactors.
- An override that matches several bindings, two modules each binding the key privately, is a wiring error listing every match. `.in_module::<M>()` on the override picks one; `.everywhere()` replaces all of them explicitly. Replacing several by default would make a test pass for the wrong reason.
- An override can't change a key's kind (single or collection). `override_many` replaces an entire collection.
- `override_factory` and `override_try_factory` follow §4's split: the plain one binds the future's output, the `try_` one its `Ok` type.
- `replace_module` swaps by identity. The replacement must export a superset of the original's keys, or wiring reports what's missing.

---

## 12. What is refused, and where

| Refusal | When | Mechanism |
|---|---|---|
| A field or parameter that isn't a site | compile | `Site` bound + `on_unimplemented` |
| `Dep<T>` with `T` not `Send + Sync` | compile | bound on `Dep`; the auto trait's own message, no hint |
| `Ext` / `ExecutionRef` in an explicit singleton | compile | `AllowedIn<S>` |
| A hook on an explicit execution-scoped or transient type | compile | `Construct<Scope: HookCapable>` |
| A closure hook on a non-singleton factory | compile | typed binding handle |
| An enhancer that lacks its role trait, including a controller-level one for any handler's transport | compile | `Guard<T>` and similar bounds, one per handler, naming the handler |
| A constructor that isn't `Send` | compile | generated impl |
| Two or zero constructors, a bad attribute | compile | macro span error |
| Trait binding with a non-implementing type | compile | coercion in the closure |
| `?` on a value whose expression failed | startup (`wire`) | `try_value` record |
| Missing dependency | startup (`wire`) | visibility pass |
| `dyn Repo` bound as `dyn Repo + Send + Sync`, or the reverse | startup (`wire`) | missing-dependency report naming both spellings |
| Two sources for one key | startup (`wire`) | visibility pass, naming both |
| Duplicate single binding, single/collection mix | startup (`wire`) | binding pass |
| Dependency cycle, module import cycle | startup (`wire`) | DFS with path |
| Singleton → execution dependency, including through transients | startup (`wire`) | needs-execution pass |
| A hook on an `Auto` type inferred per-execution | startup (`wire`) | scope pass |
| A non-optional input read on a path from a transport that doesn't seed it | startup (`wire`) | per-handler input check |
| An input declared by a keyed module | startup (`wire`) | module graph pass |
| An override that matches nothing, or more than one binding | startup (`wire`) | test builder |
| A constructor, factory, readiness check or hook fails | startup (`connect`) | typed `StartupError` |
| `ModuleRef::get` for a singleton not yet built | startup (`connect`) | `LookupError::NotReady` |
| A lazy module with controllers, contributions to a collection it doesn't introduce, or global exports | runtime (`load`) | typed error |
| Lookup not found, not ready, wrong type, wrong kind, no execution, ambiguous module | runtime | `LookupError` |
| An execution input not seeded by a standalone execution, an extension not written | runtime | `LookupError::NotFound`, or `None` through `Option` |
| A guard's refusal | runtime | `GuardRejected` through the error handlers |

---

## 13. The integration crate surface (no macros)

This is a complete database integration written against the public API:

```rust
use fw_core::{Module, ModuleDef, ModuleIdentity, Dep, Secret};

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct DbModule { url: Secret<String>, max_conns: u32 }

impl DbModule {
    pub fn for_root(url: impl Into<Secret<String>>) -> Self {
        Self { url: url.into(), max_conns: 10 }
    }
}

impl Module for DbModule {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_value(self).label("DbModule")
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        let (url, max) = (self.url.clone(), self.max_conns);
        m.secret(&self.url);                         // redacted from every error this graph reports

        m.singleton(move || {
            let url = url.clone();
            async move { PgPoolOptions::new().max_connections(max).connect_lazy(url.expose()) }
        })
        .ready(|pool: Dep<PgPool>| async move { sqlx::query("select 1").execute(&*pool).await.map(drop) })
        .retries(5)
        .timeout(Duration::from_secs(10))
        .on_destroy(|pool: Dep<PgPool>| async move { pool.close().await });

        m.contribute::<dyn HealthIndicator>()
            .singleton(|pool: Dep<PgPool>| async move { PgHealth::new(pool) }, |a| a);

        m.export::<PgPool>();
    }
}
```

The value API an integration writes against:

| Area | API |
|---|---|
| Modules | `Module`, `ModuleIdentity`, `ModuleDef::{import, global, export, reexport, secret, on_init, on_destroy, meta}`, `DynamicModule`, `Keyed` |
| Bindings | `provide::<T: Construct>`, `provide_with::<T>(factory)`/`try_provide_with`, `value`/`try_value`, `singleton`/`try_singleton`, `execution`/`try_execution`, `transient`/`try_transient`, `contribute::<T>()`, `alias::<T, Q>().of::<Existing>()`, `input::<T>().seeded_by::<Tr>()` |
| Binding handles | `also_as`, `qualified::<Q>`, `ready(..).retries(..).timeout(..)`, `on_init`/`on_destroy`/`before_shutdown`/`on_shutdown` (singleton handles only) |
| Sites | `Site`, `SiteDesc`, `Key`, `Resolver::{dep, many, entries, ext, input, module, execution, by_key}` |
| Construction | `Construct`, `Hooks<T>::{on_module_init, on_application_bootstrap, on_module_destroy, before_application_shutdown, on_application_shutdown}`, `hooks!`, `ConstructError` |
| Transports | `Transport`, `Controller::mount`, `Mount`, `Execution::{open, seed, handle}`, `ExecutionRef`, `EnhancerSpec`, the `Erased*` role twins |
| Runtime | `Timer`, `Signal`, `Server` (implemented by transports for `bind`), `AppHandle` |
| Errors | `StartupError`, `WiringErrors`, `LookupError`, `GuardRejected`, `Secret`, `Redacted` |

---

## 14. Decisions taken with defaults (open to change)

1. **Collection and extension spelling.** `Dep<T>` stays the single-dependency spelling. Collections are `Many<T>` and execution views are `Ext<T>`, because they read differently and fail differently.
2. **The qualifier is a second type parameter:** `Dep<PgPool, Replica>`, defaulting to `()`.
3. **Collections are app-wide**, not limited by visibility.
4. **Module structure is synchronous.** Registration can't await. Anything that needs async configuration becomes an async factory. This keeps "no I/O before wiring completes" true.
5. **Panics in user code are caught** at poll boundaries and reported as startup errors.
6. **Singletons are eager.** They're built during `connect`, never lazily on first use.
7. **Auto scope** is singleton for providers and inferred for controllers and enhancers. An explicit `singleton` opts out of inference.
8. **Collection order** is depth-first post-order over imports from the root, imports in the order written, then declaration order inside a module. Hooks and readiness checks tie-break on the same order.
9. **The transport owns the execution**, or `execute` does for a standalone one; everything else holds a clone. Cancellation is fired by the transport while clones are alive. The execution ends, and its instances drop, when the last clone does.
10. **`Many<T>` is eager.** Reading it constructs every contribution. The lazy form, `Resolver::entries::<T>()`, is a transport's surface, not a site.
