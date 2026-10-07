# Dependency injection: end-to-end design

`fw` is a placeholder for the framework's crate prefix. Numbers in brackets like [12] refer to the capability list in the brief.

---

## 0. Principles

1. **One runtime container, validated as a whole graph before anything exists.** Every binding declares its injection points statically, so the complete dependency graph is known before a single instance is built. All wiring errors are reported in one pass [44]. Anything that is local to one type (the type of an injection point, hooks, role bounds) is checked by the compiler instead [45].
2. **Macros are sugar.** Every macro expands to calls on the value-level API in §13, which integration crates call directly.
3. **Naming.** Types are nouns (`Dep`, `Many`, `Ext`, `ModuleRef`, `Execution`). A trait is named after its predominant method, as std's `Clone::clone` and `FromStr::from_str` are: `Construct::construct`, `FromContainer::from_container`, `Validate::validate`. A trait with no single method is named for its concept or role (`Iterator`, `Error`): `Module`, `Timer`, `Server`, `Transport`, `Guard`, `Interceptor`, `ErrorHandler`. An error is named after its operation as users understand it, so `FromStr` fails with `ParseIntError` and `Construct` with `ConstructError`; after its domain when a family of operations shares it (`io::Error`; `LookupError`, which every resolver read produces); and for the condition it reports when it reports one rather than an operation's failure, the class std has in `PoisonError`, written in the past participle: `GuardRejected`, `PanicRecovered`, `TimerMissing`. `Closed` stays, its subject implied wherever it appears; `Redacted` is a wrapper named for what was done to its contents. `From*` builds `Self` from a source and `Into*` consumes `self`; `Try*` appears only beside an infallible sibling, so a fallible method with none returns `Result` under the plain name.
4. **The core has no runtime.** It uses `std::future`, a `BoxFuture` alias, and a pluggable `Timer` trait (§3.9). Transports and runtime adapters (such as `fw-tokio`) bring the executor, sockets, timers and signals.
5. **Identity is by type.** Keys are `TypeId`s, so a type alias or a renamed import reads the same binding [9][16].

---

## 1. Crate layout

| Crate | Contents |
|---|---|
| `fw-core` | Keys, bindings, injection points, scopes, executions, modules, lifecycle, errors, and enhancer traits generic over a transport |
| `fw-macros` | `#[injectable]`, `#[construct]`, `#[module]`, `#[routes]` |
| `fw-http`, `fw-ws`, `fw-rpc`, `fw-grpc` | Transport marker types, contexts, handler attributes, middleware, servers |
| `fw-tokio` (and others) | `Timer` implementation, OS signal future, spawn helpers |
| Integrations (`fw-sqlx`, `fw-redis`, `fw-config`, ...) | Ordinary modules written against the value API |

---

## 2. The model at a glance

- **Key** is the pair of a type and a qualifier: `Key::of::<PgPool, Replica>()`. The qualifier defaults to `()`.
- **Binding** holds a key, a kind (single or contribution), a declared scope (`Auto` where none is written, §3.3), a recipe (construct, value, factory, alias), its declared injection points, its origin module, and its source location (captured with `#[track_caller]`).
- **Injection point** is a field or parameter the container fills. Its type implements `FromContainer`: it describes the key(s) it reads and knows how to read itself. `Dep<T, Q>`, `Many<T, Q>`, `Ext<T>`, `Option<S>`, `ModuleRef`, `ExecutionRef`, plus any type an integration crate defines.
- **Module** is a Rust type with an identity. It declares imports, bindings, controllers and exports.
- **Graph** is the frozen, validated result of registering every module.
- **App** is the graph plus the singleton store. It moves through typestates: `Wired` → `Connected` → `Bound` → serving → closed. From `Connected` on it hands out an `AppHandle`, a `Clone + Send + Sync` view of the shared state that outlives `serve`.
- **Execution** is one call. It holds a per-execution cache, an extension bag, execution inputs (the request or call context), a cancellation signal, and an optional deadline. The framework or the transport holds it; everything else holds a cheap-clone handle to it. Cancellation fires while handles are alive: for a call when the client disconnects or its deadline passes, for a standalone execution at its deadline, and for either at the end of the drain (§9.5). The execution ends when the last handle drops.

The lifecycle runs like this:

```
register modules (sync)
   → wire: build graph + validate everything      [no instances, no I/O]
   → connect: singletons in dependency order,
              readiness checks, init hooks, bootstrap hooks
   → bind: transports bind sockets
   → serve: each call opens an Execution
   → close(signal), or the signal passed to serve, whichever comes first:
              before-shutdown → stop accepting → drain → cancel + abandon the rest
              → destroy → sockets closed → shutdown
```

---

## 3. Core types and traits

### 3.1 Keys and qualifiers

```rust
/// A type as a report names it: `TypeName::of::<my_app::User>()` prints `User`, and with `{:#}`
/// prints `my_app::User`. Two names compare and hash on their `TypeId`s, so a type alias or a
/// renamed import names the same type; the `type_name` string is how the name prints and nothing else.
#[derive(Clone, Copy)]
pub struct TypeName { id: TypeId, name: &'static str }

impl TypeName {
    pub fn of<T: ?Sized + 'static>() -> TypeName;
    /// The names among `names` whose short form another, different type among them shares,
    /// `a::Config` beside `b::Config`: the ones a report writes with `{:#}` (§10.1).
    pub fn colliding(names: impl IntoIterator<Item = TypeName>) -> HashSet<TypeName>;
}
// Display: "User", "Arc<PgPool>"; `{:#}`: "my_app::User", "alloc::sync::Arc<my_app::db::PgPool>"

#[derive(Clone, Copy)]
pub struct Key { ty: TypeName, qualifier: TypeName }
// PartialEq, Eq and Hash are written by hand over the two names, which compare on their
// TypeIds. `type_name` is not a const fn, so each name is read at `of`.

impl Key {
    pub fn of<T: ?Sized + 'static, Q: 'static>() -> Key;
    pub fn type_name(&self) -> TypeName;   // the type the key binds, without its qualifier
}
// Display: "PgPool", "PgPool @ Replica"; `{:#}`: "my_app::db::PgPool @ my_app::Replica". A report
// decides full paths per `TypeName`, not per key: `Store @ a::Replica` beside `Store @ b::Replica`.
// `KeyName` is a `Key` with its `BindingKind`, "dyn Plugin (collection)", the form the errors carry.
```

A qualifier is any `'static` type, usually a unit struct: `pub struct Replica;`. Because keys are types, there are no strings to misspell [9]. A `Key` is also a runtime value: an integration crate that holds one can look it up erased through `Resolver::by_key::<T>(key)`, which is the one lookup that can ask for a `T` the key does not hold (§10.2). A `TypeName` is what a report holds for a type that is never bound, a transport's marker type or the `T` an extractor reads; a `Key` is a binding's identity.

### 3.2 Injection points

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

`FromContainer` is how every injection point is read. Fields, constructor parameters, factory parameters and closure parameters all go through it, which is what gives them one rule [11]. The name follows the `From*` rule (§0): the type is built from the container, as a `FromStr` type is built from a string, and the diagnostic says what to write instead, so nobody implements it for `PgPool` themselves. The transport layer's `FromCall` is its pair, a parameter built from the call, and the two names tell a reader of a diagnostic which source a parameter comes from.

```rust
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be obtained from the container",
    label = "the container cannot provide this",
    note = "write `Dep<{Self}>` to share the container's instance",
    note = "or set it inside the #[construct] fn / mark the field #[injectable(default)]"
)]
pub trait FromContainer: Sized + Send + 'static {
    /// What this type reads, for the wiring pass: key, kind, optional, needs-execution.
    fn describe(req: &mut Requirement);

    /// Build the value from the container.
    fn from_container(r: &Resolver<'_>) -> impl Future<Output = Result<Self, LookupError>> + Send;
}

/// What one injection point reads. `describe` fills it: `dep(key)`, `many(key)`, `ext::<T>()`,
/// `execution()`, `module()`, and `optional::<S>()` for everything `S` reads, marked optional.
pub struct Requirement { /* reads */ }

/// The declared injection points of a binding or a closure, one `Requirement` each, which the
/// wiring pass resolves. `field` and `param` carry a name for diagnostics; `add` is the unnamed
/// form, for a closure parameter, named by its position.
pub struct Dependencies { /* records */ }
impl Dependencies {
    pub fn field<S: FromContainer>(&mut self, name: &'static str) -> &mut Self;
    pub fn param<S: FromContainer>(&mut self, name: &'static str) -> &mut Self;
    pub fn add<S: FromContainer>(&mut self) -> &mut Self;
}
```

Implementations in the core:

| Type | Reads | Notes |
|---|---|---|
| `Dep<T, Q>` | single binding `T @ Q` | the shared `Arc` |
| `Many<T, Q>` | every contribution to `T @ Q` | collection order; eager |
| `Option<S>` | `S`, or `None` where `S` would fail with `NotFound` | optional deps [13] |
| `Ext<T>` | extension `T` of the current execution | needs an execution |
| `ModuleRef` | handle to the enclosing module [37] | |
| `ExecutionRef` | a handle to the current execution | needs an execution |

**Collection order** is one rule for every reader of a collection: depth-first post-order over imports from the root, imports in the order written, then declaration order inside a module. A module's contributions follow those of everything it imports, and two modules with no import edge between them are ordered by where the walk reaches them.

`Many<T>` is the eager form: reading it constructs every contribution. Transports read a role collection lazily through `Resolver::entries::<T>()`, which yields one handle per contribution in collection order, each with its own `resolve().await`; §7's pipeline is written against it.

A hand-written read goes through the `Resolver`: `r.dep::<PgPool>()` reads `T @ ()`, and a qualifier is a second method, `r.dep_qualified::<PgPool, Replica>()`, since a function's type parameters take no defaults and a `Q` on `dep` would break the plain spelling. `many` and `many_qualified` pair the same way; `entries` takes no qualifier, as role collections are unqualified.

`Option<S>` is `None` exactly when `S` would fail with `LookupError::NotFound`: a key no module binds, an extension no guard has written, an input this execution did not seed. Every other error propagates: a construction failure, `ExecutionRequired`, an ambiguous key or module.

Execution inputs, meaning the request or call context, are ordinary `Dep<T>` injection points. The transport declares the key as an *input* (§6.4). Integration crates can add their own `FromContainer` types, for example `fw-ws` defines `Session<T>` for per-connection state.

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
    label = "this injection point needs an execution",
    note = "declare the type #[injectable(execution)] or #[injectable(transient)]"
)]
pub trait AllowedIn<S: Scope> {}         // implemented per injection-point type and scope
// `{Self}` is the injection point's type; the type that cannot read it is the one declared `{S}`.
```

What `Auto` means depends on the binding's role:
- For a **provider**, `Auto` behaves exactly like `Singleton`. Depending on execution data is refused [21].
- For a **controller or enhancer**, `Auto` resolves to singleton if nothing below it needs an execution, and to per-execution otherwise [22][24][26].

Writing `#[injectable(singleton)]` explicitly on a controller opts out of this inference, so a controller declared that way that needs execution data is refused.

Scope is one axis and construction is another. How a binding is built is a type, a value (`value = expr`) or a closure (`with = closure`), and that is all the spelling says; a value is built once where it is written and has no scope to choose. How long a type's or a closure's instance lives is its scope: `Auto` unless one is written, with the two meanings above. The same closure means the same thing on a method, on a controller impl, in an `into` list, in a `providers` list and through the value API. One override serves every position, mirroring `#[injectable(singleton | execution | transient)]` on a type:

| Where | `Auto` | Explicit |
|---|---|---|
| `#[guards]`, `#[interceptors]`, `#[error_handlers]` (§7) | `with = closure` | `with(singleton \| execution \| transient) = closure` |
| an `into K: [..]` list (§4) | `with = closure` | the same three |
| a `providers` list (§4) | `with = closure`, the bare closure, or `K: with = closure` under a trait key | the same three |
| `ModuleDef` and `Contribute` (§13) | `with` / `try_with` | `singleton` / `execution` / `transient` and their `try_` forms |
| `EnhancerSpec` (§13) | `guard_with(f)`, `interceptor_with(f)`, `error_handler_with(f)` | `guard_with_in::<S>(f)`, `interceptor_with_in::<S>(f)`, `error_handler_with_in::<S>(f)`, `S` one of the markers above |

An explicit `singleton` that needs an execution is refused at `wire()`, type or closure alike [21] (§6.2).

Inference reads what a closure reads, not what it does. `|| RequestTimer::start()` reads nothing, so `Auto` builds it once and every call shares the one timer. A closure that creates per-call state writes `with(execution)`: that is the case the explicit form exists for, and the one inference cannot see.

### 3.4 Construct

```rust
pub trait Construct: Sized + Send + Sync + 'static {
    type Scope: Scope;

    /// How long `construct` may take (§3.9). `#[injectable(timeout = ..)]` writes `After`.
    const CONSTRUCT_TIMEOUT: Bound = Bound::Default;

    /// Declared injection points, used by the wiring pass. Generated from the fields or ctor params.
    fn dependencies(d: &mut Dependencies);

    /// Build the instance. Async and fallible [15].
    fn construct(r: &Resolver<'_>)
        -> impl Future<Output = Result<Self, ConstructError>> + Send;

    /// Lifecycle hooks this type implements. The macro fills this in by probing;
    /// a hand-written impl calls the `Hooks<Self>` methods it implements (§9.1).
    fn hooks(_h: &mut Hooks<Self>) {}
}
```

`Construct` is not generic over the resolver. `dependencies()` is the single source of truth for what a type reads.

`CONSTRUCT_TIMEOUT` bounds `construct` wherever the instance is built: during `connect` for a singleton, and inside the call for an execution-scoped or transient binding, where a hanging constructor would block the call. `Default` is the app's `construct_timeout`, 30 s. Expiry reports as `FailureReason::TimedOut`, on `ConnectError::Construct` during `connect` and on `LookupError::Construct` inside a call (§10.2). A factory takes the same bound through `.timeout(..)` on its binding handle (§9.1).

`ConstructError` (§10.2) separates a dependency's failure from the constructor's own. `Dependency(LookupError)` is a dependency read that failed: an input the execution did not seed, `ExecutionRequired`, a nested construction. The core passes it through unchanged, and what the caller gets is the dependency's own error naming the deeper key; an error naming the outer constructor would point at the wrong place. `Failed(BoxError)` is the constructor's own error, the one reported as `Construct { reason: Errored(..) }`. `From<LookupError>` is implemented and no blanket `From<E: Error>` is: `LookupError` implements `Error`, and the two impls would overlap (E0119). `?` on a dependency read is written as is, and the constructor's own error goes through `ConstructError::failed(e)`. `#[injectable]` writes that mapping for a `new` or `#[construct]` fn returning `Result<Self, E>`; a hand-written impl writes `.map_err(ConstructError::failed)?` (§13).

### 3.5 Lifecycle traits

```rust
pub trait OnModuleInit: Construct<Scope: HookCapable> {
    const TIMEOUT: Bound = Bound::Default;
    fn on_module_init(&self) -> impl Future<Output = Result<(), BoxError>> + Send;
}
pub trait OnApplicationBootstrap: Construct<Scope: HookCapable> {
    const TIMEOUT: Bound = Bound::Default;
    fn on_application_bootstrap(&self) -> impl Future<Output = Result<(), BoxError>> + Send;
}
pub trait OnModuleDestroy: Construct<Scope: HookCapable> {
    const TIMEOUT: Bound = Bound::Default;
    fn on_module_destroy(&self) -> impl Future<Output = ()> + Send;
}
pub trait BeforeApplicationShutdown: Construct<Scope: HookCapable> {
    const TIMEOUT: Bound = Bound::Default;
    fn before_application_shutdown(&self, signal: &Signal) -> impl Future<Output = ()> + Send;
}
pub trait OnApplicationShutdown: Construct<Scope: HookCapable> {
    const TIMEOUT: Bound = Bound::Default;
    fn on_application_shutdown(&self, signal: &Signal) -> impl Future<Output = ()> + Send;
}
```

`TIMEOUT` bounds the hook (§3.9): `Default` is the app's `hook_timeout`, 10 s, `After(d)` an explicit bound, `Unbounded` none. Each trait carries its own const, so a type implementing two hooks bounds each separately (§9.1).

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

`register` returns `()` and never returns early. A value that may fail to build goes through `m.try_value(result)`, which records an `Err` on the `ModuleDef` for `wire()` to report beside every other wiring error, under a `WiringErrors` entry naming the module and the key; a contribution's goes through `Contribute::try_value(result)` (§4), recorded the same way under the collection key. Stopping at the first failure would hide the rest, which is what [44] rules out.

Identity is the module's `TypeId`, plus its configuration value if it has one, plus a qualifier if it's keyed. Two configurations of one type are two modules. The same configuration imported twice is one module [31].

Diagnostic names are the type name, the key and an optional label, as in `DbModule @ Replica`. The second and later module of one type and qualifier, in collection order, carries `#n`, labelled or not: `DbModule #2`. A label names every configuration of an integration alike, and two modules printed alike cannot be told apart. The configuration's `Debug` output is never printed, because it may hold credentials.

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

/// The role keys. Sealed: `AnyGuard<T>`, `AnyInterceptor<T>` and `AnyErrorHandler<T>` implement it
/// for every `T: Transport`, and nothing else does.
pub trait Role: sealed::Sealed + Send + Sync + 'static {}
```

`Guard<Http>` and `Guard<Rpc>` are different traits, so an HTTP guard and an RPC guard are different roles [25]. A role comes only from a trait implementation. Which contributions are enhancers is decided by `TypeId` at freeze: mounting a handler records the three role keys of its transport, `AnyGuard<T>`, `AnyInterceptor<T>` and `AnyErrorHandler<T>`, and every contribution whose key is one of them is an enhancer, however it was registered (§7). `ModuleDef::enhancer::<R: Role>()` marks a contribution explicitly, for a role key of a transport with no mounted handler, and its bound refuses a type that is not a role key at compile time. Detection reads no type name and survives a compiler changing `type_name`'s format.

`Cx` wraps an `ExecutionRef` together with the call's wire data, and every clone is the same call. Everything a guard or interceptor writes goes through a shared reference and interior mutability, which the extension bag already uses. A reply that streams carries a clone of `Cx`, which is how a body, a reply stream or a WebSocket item sequence keeps its per-execution instances and its cancellation signal alive past `intercept`'s return. A request body stream lives on `Cx` behind a take-once slot, not as an execution input: inputs are `Sync` (§6.4) and a body stream is read once.

### 3.8 Execution

```rust
pub struct Execution { /* Arc: cache, extensions, inputs, cancel, deadline, module */ }   // not Clone
pub struct ExecutionRef { /* a clone of that Arc */ }   // Clone + Send + Sync

impl Execution {
    pub fn extensions(&self) -> &Extensions;               // typed bag [23]
    pub fn cancelled(&self) -> Cancelled<'_>;              // future, runtime-free
    pub fn is_cancelled(&self) -> bool;
    pub fn draining(&self) -> Draining<'_>;                // future: resolves when the app stops accepting (§9.5)
    pub fn is_draining(&self) -> bool;
    pub fn deadline(&self) -> Option<Instant>;
    pub fn handle(&self) -> ExecutionRef;                  // an owned clone, for a spawned subtask
    pub fn seed<T: Send + Sync + 'static>(&self, input: T);
    pub async fn get<T: ?Sized + Send + Sync + 'static>(&self) -> Result<Dep<T>, LookupError>;
}
// ExecutionRef has the same methods except `seed`.
```

`Execution: Send + Sync`, and `Resolver<'_>` is `Sync` with it. That is what lets `FromContainer::from_container` and `Construct::construct` hold the resolver across an await and still return `Send` futures; a `!Sync` value anywhere in the execution would make every read's future `!Send`, reported at the trait's `+ Send` rather than at the value. `seed` is bounded `Send + Sync` for the same reason.

`seed` is on `Execution` alone, the holder's handle. A guard or a subtask holds an `ExecutionRef`, and an input seeded there would be one the wiring pass never saw. `Execution` is not `Clone`; a second holder takes `handle()`.

Who holds the execution: the transport for a call, `execute` for a standalone one. Everything else holds a clone, `ExecutionRef` directly or inside a `Cx`. Three events are distinct:
- **Draining** is the notice. `draining()` resolves and `is_draining()` turns true when the app stops accepting (§9.5), the moment the transports send GOAWAY and close idle keep-alives. Both are false through the before-shutdown stage, where traffic is normal. A streaming reply that sees it finishes what it has and ends; the synchronous form is for a loop that checks between items rather than racing a future.
- **Cancellation** fires while clones are alive, so a streaming reply observes it. The transport fires it for a call when the client disconnects or the deadline passes, a standalone execution's fires at its deadline, and the drain's end fires it on every execution still alive, transport-opened and standalone alike (§9.5). The drain's end is `drain_timeout` or, earlier, the moment the `shutdown_timeout` cap expires: the same source, not a fourth. Completion of `execute` does not fire it, as finishing a response does not cancel a streaming body. A subtask that should stop when the job finishes is awaited by the job, not detached.
- **End of execution** is when the last clone drops. The per-execution cache is released then and execution-scoped instances are dropped: a streaming reply keeps its instances exactly as long as it runs.

`get` resolves against the visibility of the execution's module: the dispatching controller's module inside a transport call, the root module in a standalone execution opened on the app, or the module whose `execute` opened it (§8.2).

The execution cache uses a runtime-agnostic async once-cell (for example from `async-lock`, which is not a runtime). If two injection points in one execution resolve the same key concurrently, they get one instance. `Cancelled` and `Draining` are implemented in the core with a waker list. The core stores the deadline; the transport's runtime enforces it for a call, and `execute` enforces a standalone one on the app's `Timer` (§6.3).

### 3.9 Timer and bounds

```rust
pub trait Timer: Send + Sync + 'static {
    fn sleep(&self, d: Duration) -> BoxFuture<'static, ()>;
    fn now(&self) -> Instant;
}

pub enum Bound {
    Default,            // the app's default for this kind of bound
    After(Duration),
    Unbounded,
}
```

The core has no clock of its own. Every wait it times, the drain, a hook, a construction, a readiness check, a transport's `close` at shutdown, runs through the app's `Timer`, set on the builder (§9.4). `sleep` returns a boxed `'static` future: the trait stays dyn-compatible, which `-> impl Future` is not, and a caller can spawn what it returns. `timeout(d, fut)` is a select over `sleep`. `now` sits on the same trait because deadlines and sleeps have to read one clock; measured with `std::time::Instant`, a deadline on a paused test clock never arrives while sleeps keep resolving.

When a `Timer` is configured, the core binds it as a global export under `dyn Timer`, a value in its own global module, so a service writes `Dep<dyn Timer>` and sleeps without a runtime dependency. On an app with no `Timer` that injection point is an ordinary missing dependency. The binding is app configuration, not a binding to replace: `override_value::<dyn Timer>` in a test is a wiring error with the hint "set it with `TestApp::timer(..)`" (§11), and a module binding `dyn Timer` itself is two sources for one key against the core's export (§10.1 step 3).

`Bound` is how a hook, a constructor or a factory states how long it may take. `Default` takes the app's default for that kind of bound: `hook_timeout` (10 s) for a hook, `construct_timeout` (30 s) for a construction and for one attempt of a readiness check (§9.3). `After(d)` is an explicit bound. `Unbounded` is no bound, as a variant rather than `Some(Duration::MAX)`: `Instant + Duration::MAX` panics, and a deadline computed from it would turn "unbounded" into a panic in the shutdown path.

Every bounded item, a hook, a construction or a readiness check, is in one of those three states. A trait const writes the variant. A binding handle leaves the item at `Default` by writing nothing, makes it explicit with `.timeout(..)`, or `.attempt_timeout(..)` as well on a readiness check, and writes `Unbounded` with `.unbounded()` (§9.1, §9.3). The states are exclusive on one item, and the handle's type enforces it: an item whose bound is written has no `.timeout(..)` or `.unbounded()`, so a second bound does not compile (§9.1, §12).

One rule for a bound and the `Timer`:
- The app defaults apply only when a `Timer` is configured. Without one, an item left at `Default` runs unbounded.
- An explicit bound, `After(..)`, `.timeout(..)` or `.attempt_timeout(..)`, needs a `Timer` and is a wiring error without one (§10.1 step 6). So is a builder knob set on an app with none, and so is a readiness `.backoff(..)` (§9.3): a written wait that does not wait is refused like any other.
- `Unbounded` needs no `Timer` and raises no error. The outer `shutdown_timeout` still contains it (§9.5): unbounded per hook is not unbounded overall.
- The drain is the one asymmetry. Without a `Timer` it is zero-length, and live executions are abandoned at once (§9.5). Hooks run unbounded because they are the application's own code; executions wait on work the application does not control, and a drain with no bound would let one execution that never ends hold shutdown open.
- A standalone deadline (§6.3) on an app with no `Timer` is stored, reported by `deadline()`, and never fires. The rule that separates it from `.backoff` is where the fault is visible: a written wait that cannot wait is refused when `wire()` can see it. A deadline is a runtime value passed to `execute`, whose only error is `Closed`, and refusing it would widen that error for one rare case on an app `listen()` already confines to standalone use (§9.5).

Expiry drops the future. A hook or constructor dropped mid-await stops at that await with its own state as it was, and anything it spawned keeps running, because dropping a future stops no work spawned elsewhere. An execution abandoned at the drain's end is cancelled and left running instead (§3.8): transports and subtasks still hold it.

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

A `providers` list also takes the closure forms an `into` list takes: `with = closure`, a factory whose parameters are injection points and whose output type is the key, lowers to `m.with(closure)`, or `m.try_with(..)` for a closure answering a `Result`, the `Auto` pair `Contribute` has; `with(singleton | execution | transient) = closure` lowers to `m.singleton(..)`, `m.execution(..)` or `m.transient(..)`, or the `try_` form. A bare closure in a `providers` list is shorthand for `with = closure`, and so `Auto`: the same closure means the same thing in every position (§3.3). For a provider `Auto` behaves as a singleton, so what the bare form builds is unchanged; what differs is the record, which the needs-execution pass reads to name `with(execution)` in its hint (§6.2) and the hooks rule reads as a singleton's (§9.1).

A closure's output can also be provided under a trait key. The key comes first, as in an `into` list: `dyn Cache: with = |cfg: Dep<RedisConfig>| RedisCache::new(cfg)`, or `dyn Cache: with(scope) = ..`, lowers to `m.with(closure).also_as::<dyn Cache>(|a| a)` (the scoped method for `with(scope)`). As with `X as dyn T`, the concrete type stays bound in the module too, and exports decide what leaves it. The key comes first because a closure's body extends as far as it can: in `with = |cfg| RedisCache::new(cfg) as dyn Cache`, the `as` would parse as a cast inside the body. `K: [..]` contributes to a collection; `K: with = ..` provides a single binding. It is the spelling for a third-party type built by a closure, which `X as dyn T` cannot take because `X` must be a `Construct` type.

An `into K: [..]` list takes the entry forms `#[guards]` takes (§7), whatever `K` is: a type, lowered to `.provide::<T>(|a| a)`; `value = expr`, lowered to `.value(Arc::new(expr))`, the `Arc::new` written by the macro; `value = expr?`, lowered to `.try_value(expr.map(Arc::new))`, which records an `Err` for `wire()` as a `providers` list's `expr?` does; and `with = closure`, a factory whose parameters are injection points, lowered to `.with(closure, |a| a)`, or `.try_with(..)` for a closure answering a `Result`, through the same autoref call site a `providers` factory takes (below). The closure's body is wrapped as `#[guards]` wraps it: a synchronous body in `async move`, an `async` closure or an `async` block kept, and both spellings compile. A `value` is safe at module level, where it is refused on a controller impl (§7), because a module registers once. A `with` entry is `Auto` (§3.3). `with(singleton | execution | transient) = closure` writes its scope and lowers to `.singleton(..)`, `.execution(..)` or `.transient(..)`, or the `try_` form for a closure answering a `Result`; a word in the parentheses other than those three is a span error on it. Every `into` list lowers to `contribute`, and whether `K` is a role key is decided at freeze (§7).

A malformed `into` item is a span error on the item, "an `into` entry is a type, `value = expr` or `with = |..| ..`", followed by "; an expression is contributed with `value = ..`" when the item parses as an expression: a call such as `MetricsPlugin::new()`, a transport-scoped form such as `http = AuthGuard`, or a key before `=` other than `value`, `with` and `with(scope)`. A `with` whose right side is not a closure reads "`with` takes a closure whose parameters are injection points, as in `with = |cfg: Dep<MetricsConfig>| MetricsPlugin::new(cfg)`".

Async factories and third-party types [3][8]:

```rust
// Parameters are injection points. The output type is the key. A `try_` factory's output is a
// Result whose Ok type is the key; its Err reports as `FailureReason::Errored`, on
// `ConnectError::Construct` for a singleton and on `LookupError::Construct` for a
// binding built inside a call (§3.4).
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

A single binding under a role key the graph records is a wiring error on its own, with no contribution beside it: the pipeline reads enhancers through the collection alone (§7), so `AuthGuard as AnyGuard<Http>` in a `providers` list, `also_as::<AnyGuard<Http>>(|a| a)` on a binding handle, an alias of a role key, or a `provide`/`value` under one is bound and guards nothing. The report names the key and the binding, with the hint "contribute it". In `#[module]`, `X as <role key>` is refused earlier, at compile time, on the role key: "a role key takes contributions, and `as` binds a single instance; a global enhancer is written `into AnyGuard<Http>: [AuthGuard]`". That diagnostic reads the key as written: an alias such as `X as HttpGuards` passes the macro and meets the wiring error; a user's own type named `AnyGuard`, `AnyInterceptor` or `AnyErrorHandler`, or a `dyn` of a trait named `ErasedGuard`, `ErasedInterceptor` or `ErasedErrorHandler`, is refused after `as` although it is not a role key, and renaming the import binds it.

---

## 5. Injection points [11–17]

Every injection point obeys one rule: **an injection point is its type**. The container reads `T @ Q` for `Dep<T, Q>`, whichever identifier or alias the user wrote [16]. Fields, constructor parameters, factory parameters, enhancer closures, hook closures and readiness closures all accept any `FromContainer` type [11].

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

Compile-time checks on injection points:
- A field or parameter whose type is not `FromContainer` produces that trait's diagnostic, which tells the user to write `Dep<T>` or set the value in the constructor.
- `Dep<T>` requires `T: Send + Sync`. The error is the auto trait's own ("`(dyn Repo + 'static)` cannot be sent between threads safely"), with no hint: rustc reports the unsatisfied `Send` from the bound's where-clause and drops any wrapper trait's `on_unimplemented` note. The fix, `Send + Sync` as supertraits of `Repo`, is documented rather than diagnosed. The other spelling mistake, a binding under `dyn Repo + Send + Sync` read as `dyn Repo` or the reverse, compiles on both sides and is caught at `wire()` (§10.1).
- `Ext<T>` or `ExecutionRef` in a type explicitly declared `singleton` fails the `AllowedIn` bound. The macro emits one `quote_spanned!` assertion per injection point so the error points at that field or parameter.
- A constructor whose future isn't `Send` fails at the generated `Construct` impl, pointing at the function.

---

## 6. Scopes and executions [18–23]

### 6.1 Scope declarations

```rust
#[injectable]               // Auto
#[injectable(singleton)]    // explicit singleton
#[injectable(execution)]    // once per execution, shared inside it
#[injectable(transient)]    // fresh at every injection point
#[injectable(timeout = Duration::from_secs(5))]   // CONSTRUCT_TIMEOUT (§3.4); combines with a scope
```

Factories choose their scope with the method they're registered through: `m.singleton(..)`, `m.execution(..)`, `m.transient(..)`. Types that implement `Construct` always use `T::Scope`. Registration can't override it, which is what keeps the compile-time hook check sound.

### 6.2 Scope resolution at wiring time

After the cycle check, the graph computes whether each binding **needs an execution**. A binding needs one if any of its injection points is `Ext`, `ExecutionRef`, an execution input, or a dependency that itself needs an execution. Transient and auto bindings pass this need upward. The rules are then:

| Binding | Needs an execution | Result |
|---|---|---|
| Explicit singleton: `#[injectable(singleton)]`, `with(singleton) = ..`, `.singleton(..)` | yes | **refused** [21], with the full path |
| Auto provider, a type or a `with` closure under a provider key | yes | **refused**; the hint names the explicit form, `#[injectable(execution)]` for a type and `with(execution)` or `.execution(..)` for a closure |
| Auto controller or enhancer, a type or a `with` closure | yes | becomes per-execution, built per call [22][26] |
| Auto type with hooks that became per-execution | — | **refused** (the startup half of [41]); a closure hook or `.ready(..)` on its handle counts as a hook (§9.1) |
| Transient | yes | allowed, but every consumer must be able to run in an execution |

A hook, readiness, module-hook or metadata closure runs where no execution exists, so one reading `Ext<T>`, `ExecutionRef`, an input or a per-execution key is refused at `wire()`: nothing can ever satisfy it there, and `connect` is later and costlier. An enhancer closure runs inside an execution and is exempt.

### 6.3 Opening executions

Transports open one execution per HTTP request, WebSocket message, RPC call or gRPC call. A WebSocket connection is not an execution: per-connection state lives in `fw_ws::Session<T>`, a `FromContainer` type read from the execution input the WebSocket transport seeds [19].

Code opens standalone executions itself:

```rust
let report = app
    .execute(ExecOptions::new().deadline(Instant::now() + Duration::from_secs(30)), async |exec| {
        let job = exec.get::<ReportJob>().await?;
        job.run().await
    })
    .await??;                                   // `Closed` from Draining on, then the closure's own `Result`
```

```rust
impl App<Connected> {   // also on AppHandle and ModuleRef
    pub async fn execute<F, R>(&self, opts: ExecOptions, f: F) -> Result<R, Closed>
    where
        F: AsyncFnOnce(&Execution) -> R;
}
```

`execute` is refused from Draining on with `Closed`, a small public struct (§9.5). The closure borrows the execution, which stays owned by `execute`: `execute` drops it when the future completes, and a subtask that outlives the call takes `exec.handle()`, an owned clone that keeps the cache and the execution-scoped instances alive until it drops (§3.8). Completion of `execute` fires no cancellation; a standalone execution is cancelled at its deadline or at the end of the drain, and a subtask that should end with the job is awaited by the job. A plain closure returning an `async move` block cannot borrow its argument into the future, which is why the bound is `AsyncFnOnce` and the closure is written `async |exec|`. `execute` is an inherent `async fn` on every type that carries it, never a trait method: its future is `Send` through auto-trait leakage whenever the caller's closure future is, and a trait would have to write that `Send` bound, which stable Rust cannot state for an `AsyncFnOnce` future.

`App::execute` and `AppHandle::execute` resolve with the root module's visibility; `ModuleRef::execute`, reached through `app.module::<M>()?`, resolves with `M`'s (§8.2).

A standalone deadline is enforced on the app's `Timer`: `execute` polls `Timer::sleep` to the deadline beside the closure, cancellation fires when the sleep resolves and the closure runs on to its end, and a deadline already past fires before the closure first runs. On an app with no `Timer` the deadline is stored, `deadline()` reports it, and cancellation never fires from it; §3.9 states why that is accepted where a written `.backoff` is refused. `Execution::open` stores a deadline and enforces nothing: the transport that opened the execution does.

One kind of execution opens while `execute` and `Execution::open` are refused. A transport ending a connection during the drain still has that connection's cleanup to run, a gateway's disconnect handler for example, and the cleanup is itself an execution. `Execution::open_terminal(&DrainToken, ..)` opens one: allowed in Draining, counted in the drain and bounded by `drain_timeout` like any other execution. `DrainToken` has a private field and one source, the core, which hands it to each transport through `Server::drain` as Draining begins (§9.5). It is `Clone`, an `Arc` inside, because a transport opens terminal executions from connection tasks it spawned long before the drain and each needs its own copy; cloning creates no second source. The token proves who opens the execution and the phase check proves when. Anyone implementing `Server` holds one, a user-written transport included, since the rule is "only transports" and not "only these crates"; a token or a clone used after the drain has ended gets `Closed`.

### 6.4 Execution inputs

A transport declares the context types it seeds, together with the transport that seeds them, so the wiring pass knows both:

```rust
// inside fw-http's own global module
m.input::<RequestHead>().seeded_by::<Http>();
m.input::<ClientAddr>().seeded_by::<Http>();
```

At each call, the transport seeds them with `exec.seed(head)`, bounded `T: Send + Sync + 'static` (§3.8). Inputs are app-wide and belong to transports: a keyed module declaring one is a wiring error.

Wiring walks each handler's reachable execution-scoped bindings and checks every non-optional input against that handler's transport. A per-execution service reading `Dep<RequestHead>` on a path from an RPC controller is a wiring error naming the handler, the service and the input, printing the dependency path from the handler to the service that reads the input, and naming both transports: the handler's and the one that seeds the input (§10.1). A service shared across transports reads the input as `Option<Dep<RequestHead>>`, which is `None` where the transport did not seed it. An input several transports seed is declared by each of them through `Transport::inputs`, each naming the others with `also_seeded_by` (transports §2.10): the freeze merges two declarations of one key whose seeder sets are equal into one declaration carrying both seeders, and the check passes a handler of any. A module's `seeded_by` names one seeder. A second declaration of the key by another transport with a different seeder set, or a transport's declaration beside a module's with another seeder or beside a binding under the key, is the wiring error `InputConflict` (§10.1 step 2).

Standalone executions are the one runtime case, since nothing static says what they seed: one that doesn't seed an input which an injection point reads gets `LookupError::NotFound { kind: Input }` at runtime, and the error names the key.

The same walk answers a transport's `prepare`: `Mounted::handlers_reading::<T>()` lists the mounted handlers whose reachable execution-scoped bindings read the input `T`, one `InputReader` per handler and read. An entry carries the handler (`handler()`), its transport as a `TypeName` (`transport()`) and the steps after the handler (`steps()`): each binding between the handler and the read, then the injection point, as in `Audit (execution)`, ``Dep<ClientAddr> (field `addr`)``. No entry carries the handler step rendered. A refusal writes `{controller}::{handler} ({transport})` from the handler and the `TypeName` against its own report's names (§10.1), and the transport prints one way throughout that report. A transport whose deployment cannot seed an input it declares, an embedded HTTP app whose host supplies no peer address for `ClientAddr` (transports §3.8), refuses those handlers at startup naming the path to the service that reads the input, rather than failing their first call.

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
    #[guards(with = |u: Ext<CurrentUser>| RoleGuard::require(u, Role::Admin))]  // closure, Auto: per execution, it reads Ext
    #[guards(with(execution) = || RequestTimer::start())]                        // closure, explicit: fresh state per call
    async fn get(&self, id: Path<u64>) -> Result<Json<User>, HttpError> { /* ... */ }

    #[fw_rpc::message("users.get")]
    async fn get_rpc(&self, req: Payload<GetUser>) -> Result<User, RpcError> { /* ... */ }
}
```

A controller-level enhancer applies to every handler on the impl, strictly. `#[interceptors(TimingInterceptor)]` expands to `spec.interceptor::<TimingInterceptor>()` once per handler, and each expansion requires the role for that handler's transport: `Interceptor<Http>` for `get`, `Interceptor<Rpc>` for `get_rpc`. A handler whose transport lacks the role is a compile error at the attribute naming the handler: written `#[guards(AuthGuard)]`, the impl above fails on `get_rpc` with a message like "`AuthGuard` is not a guard for `Rpc`, needed by `get_rpc`; implement `Guard<Rpc>`". There is no "apply where the role exists": a guard that guards three of four handlers is the bug [25] exists to prevent. A controller that serves several transports scopes an enhancer with the transport-scoped form, `#[guards(http = AuthGuard)]`, which applies it to that transport's handlers alone and checks only their role.

`value = expr` is written per method, where it is built once at mount and shared by every call of that handler. On the impl it is a compile error, "declare it per method, or bind it by type for shared state": each handler mounts separately, an impl-level value would be built once per handler, and a rate limiter declared there would limit each handler alone while the application kept working. It stays refused until the transport protocol can share one value across the impl's handlers; a refusal that later relaxes into a feature breaks nobody. A known limit sits beside it: a controller-level entry scoped to a transport key no handler carries, `#[guards(htpp = AuthGuard)]`, applies to nothing and is not yet reported. `#[routes]` expands before the transport attributes and does not know its handlers' keys; the report waits for the transport protocol to carry each transport's key.

In a `#[routes]` impl, a method carrying any attribute other than the language's own (`doc`, `allow`, `warn`, `deny`, `expect`, `cfg`, `cfg_attr`, `inline`, `must_use`, `deprecated`, `track_caller`) or an enhancer attribute is a handler. A helper annotated `#[tracing::instrument]` is read as one and fails to compile, as a handler whose transport attribute read nothing; helpers go in a separate `impl` block.

Global enhancers are collection contributions under a role key [28]. In `#[module]` they are an `into` list, `into AnyGuard<Http>: [AuthGuard, value = RateLimit::per_second(100)]`, which lowers to `contribute` like every `into` list (§4); the value API writes `contribute` or `enhancer`:

```rust
m.contribute::<AnyGuard<Http>>().provide::<AuthGuard>(|a| a);   // a global HTTP guard
m.enhancer::<AnyInterceptor<Rpc>>().value(Arc::new(Tracing::default()));
```

The key decides the role, at freeze. `Mount::handler` records the `TypeId`s of its transport's three role keys. At the end of the freeze, once every controller has mounted its handlers, the role set is those keys for every mounted handler, a lazy load's base included, plus the key of every contribution marked through `enhancer`. Every contribution whose key's `TypeId` is in the set is an enhancer, through `contribute` or `enhancer`, under an alias or a `macro_rules` wrapper alike: scope inference (§3.3) applies to it, and no type name is read. One marked contribution marks the whole key, so one `m.enhancer::<K>()` anywhere in the graph marks every `into` list's contributions to `K` with it. The same pass refuses two forms under a key in the set, each of which nothing reads: a qualified contribution, `contribute::<AnyGuard<Http>>().qualified::<Q>()`, registered under `AnyGuard<Http> @ Q` where the pipeline walks `entries::<AnyGuard<Http>>()` unqualified (§3.2), with the hint "contribute it unqualified"; and a single binding, through `also_as`, an alias, `provide` or `value`, which the collection read never reaches, with the hint "contribute it" (§4). A role key no mounted handler reads and no `enhancer` marks is outside the set and is not checked.

`enhancer::<R: Role>()` is the explicit mark, for a role key of a transport with no mounted handler, which the recorded set cannot see: a shared auth module wired into a worker binary, or a test app without controllers. Unmarked, that contribution would be a provider, and an `Auto` entry needing an execution would be refused as a singleton (§6.2). `#[module]` has no spelling for the mark; an `into` list lowers to `contribute`. A module contributing enhancers for a transport the app mounts no handler of implements `Module` by hand, or relies on one `m.enhancer::<K>()` contribution written anywhere in the graph, which marks the key for the list's contributions too. The `Role` bound (§3.7) refuses a type that is not a role key. `enhancer` returns the builder `contribute` returns, marked: `Contribute<'m, U, Q, Mark = Plain>`, `Plain` from `contribute` and `Enhancer` from `enhancer`, every method shared across the mark but `qualified`, which exists on `Plain` alone. `Plain` and `Enhancer` are empty enums, exported at `fw::handle` beside the binding handle's state markers: no value of either exists, and code generic over the mark names no sealed trait. A role collection is read unqualified (§3.2), so `.qualified::<Q>()` on an enhancer contribution is E0599 at the call; the same qualifier through `contribute` is the wiring error above.

A `with` closure takes its scope from the axis in §3.3: `Auto` unless `with(scope) = ..` writes one, and the same moved between a method, the impl and an `into` list. `#[guards(with = f)]` lowers to `spec.guard_with(f)` and `#[guards(with(execution) = f)]` to `spec.guard_with_in::<scope::PerExecution>(f)`, `#[interceptors]` and `#[error_handlers]` to their kin, and an `into` list's entry to `Contribute::with` or the scoped method (§4). An `Auto` closure is inferred as a type is, from what its parameters read (§6.2): `with = |u: Ext<CurrentUser>| ..` above is built per call because it reads `Ext`, and the `RequestTimer` guard reads nothing and writes `with(execution)` to be built per call (§3.3).

Stack order is global, then controller, then method [26]. Within the global level, contributions follow collection order (§3.2).

The dispatch pipeline for one call [27] runs like this. Steps 2 to 4 are the core's `dispatch(handler, exec, cx, call)`, which every transport calls rather than writing the walk itself:

1. The transport opens an `Execution`, seeds the inputs, and builds its `Cx` around a handle to it.
2. `dispatch` reads the role collection through `Resolver::entries::<AnyGuard<T>>()`, which hands it one unresolved entry per contribution, and walks them in stack order together with the controller's and the method's declarations. For each guard it obtains that guard (a shared value, the singleton, `resolve().await` for a per-execution build, or the closure call), then runs `can_activate`. On a refusal it stops. **Later guards are never built.** `Many<AnyGuard<T>>` would build every guard first, which is why the pipeline reads `entries` and not `Many`.
3. Only once every guard admits does it build the interceptors, then the controller (per call if inferred), then run the handler inside the interceptor chain.
4. Errors go through the error handlers, method level first, then controller, then global. A refusal is one of them: the pipeline turns `Ok(false)` into the core error `GuardRejected { guard: &'static str }` and routes it like any other, so a handler that downcasts the `BoxError` can reshape the response per route. Unclaimed, the transport renders its forbidden status: 403 on HTTP, `PERMISSION_DENIED` on gRPC, an error reply on RPC, an error frame on WebSocket. Each transport crate documents its mapping. A guard that wants another status, 401 rather than 403, returns its own `Err`. A panic is one of them too. Each stage runs under its own catch, a guard, an interceptor, the handler and each error handler, and a panic becomes `PanicRecovered { stage, message }` (§10.2) where it happened, `stage` a `DispatchStage` naming which of the four; a `with` closure's panic counts as the stage of the enhancer it builds. A handler's panic passes back through the interceptors: the innermost interceptor's `next.run()` answers `Err(PanicRecovered { stage: Handler, .. })`, each interceptor further out receives what the one inside it answered, and an interceptor reshapes or answers it as it does any handler error. State the panicking stage touched may be inconsistent, so an interceptor treats that `Err` as fatal for the call rather than retrying. A panicking error handler's panic is offered to the handlers after it. A panic while the container builds an enhancer or the controller, a per-execution guard's constructor for example, is no stage's and arrives as `LookupError::Construct { reason: Panicked(..) }`; `fw::is_panic(&BoxError) -> bool` answers `true` for either shape, and for one a constructor's or a `try_` factory's own `Err` wraps (§10.2); an error handler mapping every panic to one status is one line. Unclaimed, the transport renders its internal-error status.
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

A lookup that names no module uses the root module's visibility: `app.get`, and `exec.get` inside an execution the app opened, see the root's own bindings, its imports' exports and the globals. The root's table is free of ambiguity once wiring passes, so no lookup variant is needed for it. A service exported only within a subtree is unreachable that way by design; it is reached through `app.module::<M>()?`, and a job that belongs to a subtree runs through that module's `execute`. Inside a transport call, the execution's resolver carries the dispatching controller's module: handlers and their per-execution services see what their module sees. Those tables are checked only where an injection point reads, so a lookup through `ModuleRef::get`, through `exec.get` in an execution opened on a module, or through `by_key` can meet a key two of that module's imports export; it answers `LookupError::Ambiguous { key, sources }` (§10.2), and `Option<S>` propagates it rather than answering `None`.

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

`Keyed<Q, M>` is itself a module. Inside `M`, injection points stay unqualified (`Dep<PgPool>`). At the export boundary, unqualified exports are requalified as `T @ Q`, re-exports included, since they leave the boundary like any export. Contributions are not exports and keep their key: two keyed databases contribute two `dyn HealthIndicator` entries. Execution inputs are app-wide and belong to transports; a keyed module declaring one is a wiring error.

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
let reports: ModuleRef = handle.load(ReportsModule).await?;   // Result<ModuleRef, LoadError> (§10.2)
```

`AppHandle` is a `Clone + Send + Sync` view of the shared inner state, available from `Connected` on. `get`, `module`, `execute`, `load`, `close`, `draining`, `is_draining` and `drain_timeout` live on it, `draining()` returning the same `Draining<'_>` an execution's does (§3.8) and resolving at the same moment, and `drain_timeout()` the window the drain runs under (§9.5), zero on an app without a `Timer`, which a transport hands to a host that stops on a clock of its own (transports §3.8); `serve(self)` consumes only the `Bound` typestate, so a handle taken before `serve` keeps working while the app serves. `close` on a handle is one of shutdown's two triggers and ends `serve` (§9.5). The graph sits behind a lock that only `load` writes.

A lazily loaded module is wired against the frozen graph, with all of its errors reported in one pass, and then connected through its own readiness checks and init hooks. Loading the same identity twice returns the existing handle.

`load` fails with `LoadError` (§10.2): `Wiring` for the module's wiring errors, `Connect(ConnectError)` for a construction, readiness check or init hook that fails, `Refused(LoadRefusal)` for what the frozen graph cannot take, and `Closed` by phase. `LoadRefusal` names five things, refused because the graph has already been handed out:
- **Controllers and middleware**, because routes are already bound.
- **Contributions to a collection the module does not introduce itself.** A lazy module cannot contribute to any key that a pre-existing binding reads as `Many<T>`, whatever that binding's scope; the rule is static and checked at `load` without asking what has already run.
- **Global exports.**
- **Execution inputs.** Inputs belong to transports (§6.4), and the per-handler input check ran at wiring time over the inputs the graph then had.

Shutdown includes lazily loaded modules, in reverse order of loading. `load` is refused with `LoadError::Closed` from Stopping on (§9.5): a module loaded then would miss the before-shutdown stage already running, and its shutdown would be incomplete.

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
    const TIMEOUT: Bound = Bound::After(Duration::from_secs(3));   // `Default` when left out (§3.5)
    async fn on_application_shutdown(&self, signal: &Signal) { self.flush().await }
}
```

For factory outputs (such as a third-party pool), hooks are closures registered on a **singleton** binding handle, the handle of an `Auto` provider included, since an `Auto` provider is a singleton (§3.3); for modules, on the `ModuleDef` (§13). The binding handle is typed, so the hook methods and `.ready(..)` don't exist on execution-scoped or transient handles [41], and an `Auto` binding that inference moves per-execution with any of them written is refused at `wire()`, the readiness check counted as a hook (§6.2):

```rust
m.try_singleton(|cfg: Dep<DbConfig>| async move { PgPool::connect(cfg.url.expose()).await })
    .timeout(Duration::from_secs(20))                                        // bounds construction
    .on_destroy(|pool: Dep<PgPool>| async move { pool.close().await })
    .timeout(Duration::from_secs(3));                                        // bounds the hook

m.on_init(|users: Dep<UserService>| async move { users.seed_admin().await })   // module hook [39]
    .timeout(Duration::from_secs(5));
```

The handle is one type with a state parameter naming the item written last and whether its bound is written, and the binding is the first item: `.timeout(..)` and `.unbounded()` write the bound of whatever the state names, construction directly after `singleton(..)`, the check after `.ready(..)`, the hook after `.on_destroy(..)`, and either call moves the item to a state that has neither method. A second `.timeout`, or `.unbounded()` beside a bound, is E0599 at the second call (§12). `.retries`, `.backoff` and `.attempt_timeout` exist on the readiness item alone, the first two in every one of its states and repeatable, the last write winning (§9.3). `also_as` and `qualified` describe the binding and return to it in the state it was left in: the handle carries the construction's state across the other items for that return, and the construction's bound is written once wherever the handle is on the binding. Misuse is E0599 at the method, naming the handle type the method exists on and the state bound it lacks. A closure's `.timeout` means what the trait const means (§3.9), `Default` when left off, and `.unbounded()` writes `Unbounded`.

A closure hook reads its parameters before it runs, and a read that fails means the hook never ran: the failure is recorded as `FailureReason::Errored` holding the `LookupError`, on `ConnectError::Hook` for an init or bootstrap hook and on `ShutdownFailure::Hook` for a destroy or shutdown hook (§10.2).

A factory's output is a plain value, so trait hooks run only for types the container constructs. This rule is documented, and it's the reason closure hooks exist. It holds even when the output type implements `Construct`: `m.singleton(|| async { Cache::custom() })` binds `Cache` by factory, and its `OnModuleInit` impl compiles and never runs, because the graph learns which types are `Construct` only through `provide`. A `Construct` type that needs a custom build is bound with `m.provide_with::<Cache>(|| async { Cache::custom() })`, which runs `Cache::hooks` like `provide` does (§4).

`Hooks<T>` has one method per hook trait, each with a `where` bound, so `h.on_module_init()` compiles only when `T: OnModuleInit`, and likewise `on_application_bootstrap`, `on_module_destroy`, `before_application_shutdown` and `on_application_shutdown`. A hand-written `Construct` impl knows which traits it implemented and calls those methods. `#[injectable]` fills `hooks` with autoref probes over the concrete `Self`; the probe types are macro internals, and `fw::hooks!(h)` is the same probing offered as a macro for a hand-written impl that wants all five checked. The registration reads each hook's bound fully qualified, and that is also how user code reads one on a type implementing several hook traits: `<T as OnModuleInit>::TIMEOUT`; `Self::TIMEOUT` is ambiguous there.

### 9.2 Order [40]

The connect phase walks the singleton graph in a stable topological sort: among the bindings whose dependencies are done, the smallest (module post-order index, declaration index) runs next, the module index being the module's position in collection order (§3.2). For each binding it constructs the instance and runs that binding's readiness check. It then runs all `OnModuleInit` hooks in the same order. A module's own hooks run after the hooks of its providers. Last come all `OnApplicationBootstrap` hooks. Two bindings with no edge between them are ordered by the tie-break, so the order is the same on every run.

Close runs the hooks in the exact reverse, with the drain between the first and the rest: `BeforeApplicationShutdown(signal)` while the app still serves, then stop accepting and drain, then `OnModuleDestroy`, the socket close and `OnApplicationShutdown(signal)` (§9.5).

The first failure stops the walk, and what the walk built gets no hook. `connect` returns `StartupError::Connect` and drops the app: the singletons built so far, init hooks run included, run no destroy or shutdown hook, and their resources release as the instances drop. A failed `load` additionally restores the base graph and removes the singletons it built, so a retry wires again. That is a known limit, to fix before release: a drop releases pools and file handles, and not what an init hook did outside the process, a registration with service discovery, a distributed lock, a lease. A failed start that leaves the service registered as healthy is the cost. Partial teardown wants its own design, since §9.5's steps presuppose a connected app.

### 9.3 Readiness checks [42]

```rust
m.singleton(|cfg: Dep<DbConfig>| async move { PgPool::connect_lazy(cfg.url.expose()) })
    .ready(|pool: Dep<PgPool>| async move { pool.ping().await })
    .retries(5)
    .backoff(Duration::from_millis(500))
    .attempt_timeout(Duration::from_secs(2))    // one attempt
    .timeout(Duration::from_secs(10));          // the whole check, retries and backoff included
```

A readiness check runs right after its binding is constructed and before anything that depends on it. `.timeout` bounds the whole check, retries and backoff included, so it means what a hook's `.timeout` means; `.attempt_timeout` bounds one attempt and is what catches a single hung ping. Both are optional, and both need a `Timer` and are wiring errors without one (§3.9); so does `.backoff`, which waits between attempts and cannot without one, and the report names the `.backoff(..)` call, which records its own location, rather than the `.ready(..)` before it. A check that writes neither takes `construct_timeout` as its attempt bound when a `Timer` exists, under the rule constructors follow: external I/O is what a readiness check exists for, and a ping to a host that drops packets never returns, so an unbounded first attempt would never reach its retries. Without a `Timer` it runs unbounded. Writing `.timeout`, `.attempt_timeout` or `.unbounded()` opts out of that default. Each of the two is written once, in either order, and `.unbounded()` stands in for both: it exists only while neither is written, and after it neither exists (§9.1). `.unbounded()` runs the check with no bound on an attempt or on the whole, for a dependency the app waits on however long it takes; `.retries` and `.backoff` keep their meaning under it. Neither of those two is once-only: they exist in every state of the item, and a repeated `.retries` or `.backoff` is last write wins. A repeated bound would conflict with itself, which is what the typestate refuses; a repeated count is redundant, and refusing it would cost states. An attempt that times out with retries left is retried like one that returned `Err`. A check that fails reports as `ConnectError::Readiness` with the attempts made and a `reason` for how it ended (§10.2): `TimedOut { limit: Item }` when the whole-check bound fired, whatever the attempt underway was doing; `TimedOut { limit: Attempt }` or `TimedOut { limit: Default }` when the last attempt hit its own bound with no retries left; `Errored(..)` holding the last attempt's error when the retries ran out on errors; `Panicked(..)` when an attempt panicked, which ends the check at once.

Every error the core did not create itself, from user code, integrations or transports, passes through one redaction function before it is stored in any core error type, and what is stored is a `Redacted` (§10.2): the `Errored` and `Panicked` payloads of a `FailureReason`, a panic message and a transport's `close` error among them, `Bind`'s `source`, and the `Err` a `try_value` recorded in `WiringErrors`. The scope is the error's origin, not a list of variants, so a variant added later is inside it with no list to extend. The function replaces every `Secret<_>` registered with the graph and, as a backstop, strips the userinfo from anything shaped like a URL. On the `try_value` path the registry has nothing to replace: the secret that would have been registered sits inside the config that failed to load. The strip is the only protection there, a backstop and not a guarantee. A `Redacted` writes the redacted text under `Display` and `Debug` alike, answers `None` from `source()`, and hands over the original through `downcast_ref` and `into_inner` alone (§10.2). Registration is explicit per graph, never a process-wide list, which would leak between tests:
- `m.secret(&self.url)` in the value API, for a secret a module holds in its configuration and moves into a factory.
- `#[module]` registers every `Secret<_>` field of a configured module, since it can see the fields.
- `m.value(Secret<_>)` registers the value it binds.

### 9.4 Phases [43]

```rust
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let app = App::builder(AppModule)
        .timer(fw_tokio::Timer)
        .drain_timeout(Duration::from_secs(5))      // in-flight executions at shutdown; 10 s unset (§9.5)
        .shutdown_timeout(Duration::from_secs(25))  // the whole close sequence; unset by default (§9.5)
        .wire()?;                                   // graph + all wiring errors; no instances, no I/O

    let app = app.connect().await?;                 // outbound connections, readiness, hooks

    let app = app
        .bind(fw_http::Server::new("0.0.0.0:8080"))
        .bind(fw_grpc::Server::new("0.0.0.0:50051"))
        .listen()
        .await?;                                    // sockets

    let handle = app.handle();                      // AppHandle: get, module, execute, load, close, draining, drain_timeout
    app.serve(fw_tokio::shutdown_signal()).await?;  // until the signal or `handle.close(..)`; returns the `Shutdown` report
    Ok(())
}
```

`hook_timeout` (10 s unset) and `construct_timeout` (30 s unset) sit beside those two knobs and are the `Default` every hook and construction starts with (§3.9); `construct_timeout` also bounds one attempt of a readiness check left at `Default` (§9.3). All four are timed by the `Timer` and apply only with one; set on a builder with none, each is a wiring error.

A job, a CLI command or a test stops after `connect()` and uses `get` or `execute`, then calls `close(Signal::new("done"))`, which runs the shutdown sequence with no sockets to close (§9.5). Anything that needs the app while it serves holds an `AppHandle` taken before `serve` (§8.6).

### 9.5 Shutdown

Shutdown is one event per app. It has two triggers: the signal future passed to `serve` resolving, and `close(signal)` on the app or on any `AppHandle`. The first to arrive starts the sequence, and its `Signal` is the one `BeforeApplicationShutdown` and `OnApplicationShutdown` receive. `handle.close(..)` ends `serve`: the serve future runs the sequence and returns its outcome.

A `close` during a running shutdown starts nothing. It waits for the same shutdown and returns the same outcome. Its own signal is ignored, as is `serve`'s signal arriving during a handle-initiated close.

The sequence:

1. `BeforeApplicationShutdown(signal)` hooks, in reverse connect order, while the app still serves. Traffic is normal through this stage: a service that deregisters from discovery and waits for the last routed request to arrive does it here.
2. Stop accepting. `draining()` resolves and `is_draining()` turns true on every execution and every `AppHandle`. Each transport does the protocol's equivalent at the same moment: HTTP/2 and gRPC send GOAWAY, HTTP/1 closes idle keep-alive connections, and WebSocket closes idle connections with 1001 at once while a busy connection stops reading, finishes the messages it has in flight and then closes with 1001. The core hands each transport a `DrainToken` through `Server::drain`.
3. Drain. The app waits for live executions to end on their own, up to `drain_timeout`. Nothing is cancelled in this window. New executions are refused with `Closed`, except the terminal ones a transport opens with its token for a connection's cleanup (§6.3), which count in the drain like any other.
4. Cancel and abandon the rest. At the timeout, cancellation fires on every execution still alive, transport-opened and standalone alike, and each is abandoned: it keeps its cache and its instances until its last handle drops (§3.8), and the outcome counts it. The drain also ends early when the `shutdown_timeout` cap expires, and the end of the drain is then that moment rather than `drain_timeout`; cancellation has no fourth source.
5. `OnModuleDestroy` hooks, in reverse connect order. From the first of them on, a fresh singleton lookup from an abandoned execution answers `LookupError::Closed`, and an instance it already holds stays a valid object whose destroy hook may have run.
6. Transports close their sockets. Each `close` runs in reverse bind order, one after another, bounded by what is left of `shutdown_timeout`, or by `hook_timeout` when no cap is set. A `close` can wait on a TLS close-notify or a broker's acknowledgment, and one that exceeds its bound is dropped and recorded as `ShutdownFailure::Close` with `reason: TimedOut`, the limit `ShutdownCap` under the cap and `Default` under `hook_timeout` (§10.2); one that returns an error is recorded with `Errored`, and one that panics is caught and recorded with `Panicked`, as a hook's panic is. `close` is called inside the caught future, so a `Server::close` that panics before returning its future is caught too. Every `close` starts, and `Skipped` never occurs on `Close`.
7. `OnApplicationShutdown(signal)` hooks, in reverse connect order.

| Phase | New executions | Singleton lookups | `load` |
|---|---|---|---|
| Running | allowed | allowed | allowed |
| Stopping (step 1) | allowed | allowed | refused, `LoadError::Closed` |
| Draining (steps 2–4) | refused, `Closed`; terminal executions allowed | allowed | refused |
| Destroying onward (steps 5–7) | refused | `LookupError::Closed` | refused |

`is_draining()` is `false` throughout Stopping. Draining means new work is refused and the drain window has started; a stream told to end during Stopping would end before the discovery hook has deregistered, which is what the hook's position exists to prevent. Early notice is `BeforeApplicationShutdown`.

**Bounds.** Each hook runs under its own `TIMEOUT` (§3.9), `hook_timeout` where it writes none. The drain runs under `drain_timeout`, ten seconds unset. `shutdown_timeout` caps the whole sequence and is unset by default. Hooks run one after another: twenty hooks at ten seconds each take more than three minutes, while an orchestrator kills the process at its grace period, thirty seconds in Kubernetes, mid-hook and with nothing reported. The cap starts at the trigger, where the orchestrator's clock starts too, and the before-shutdown stage counts against it; a deployment sets it a few seconds under the grace period. When it expires, every remaining step that waits on user code is skipped: hooks not yet run are recorded as `Skipped`, a hook mid-run is dropped and recorded as `TimedOut` with `limit: ShutdownCap` and the cap's duration as `after` (§10.2), and a drain still open cancels and abandons at once. Stop accepting still runs, as it runs no user code. Each transport's `close` still starts, under what is left of the cap: nothing once it has expired, so a `close` that does not complete in its first poll is dropped and recorded as `ShutdownFailure::Close`. A hook declared `Unbounded` is still inside the cap.

**Abandoning a hook** drops its future (§3.9). The hook stops at its next await with its own state as it was, and anything it spawned keeps running. An execution abandoned at the drain's end is cancelled and left running instead: transports and subtasks still hold it. A hook that times out, is skipped by the cap, panics, (an init or bootstrap hook) returns an error or (a closure hook) fails to read a parameter is recorded with that reason (§10.2), and a failing step never stops the sequence. The outcome is `Result<Shutdown, ShutdownError>` (§10.2), received by `serve` and by every `close` caller. `ShutdownError` is `Clone` with its contents behind an `Arc`, because one outcome has several receivers.

**Disconnect handlers at shutdown are best effort.** A terminal execution follows the drain's rules: one opened a moment before `drain_timeout` is cancelled almost at once, and a connection still busy at the timeout has its cleanup refused, since its terminal execution would open after the drain has ended. No grace window follows the drain to save them. A handler opened at the end of such a window would have the same problem, and the same handler also never runs on SIGKILL, an OOM kill or a lost node. State that outlives the process, presence in Redis or room membership in a shared store, belongs in TTL or heartbeat storage and cleans itself up; state that dies with the process needs no cleanup. The outcome counts the terminal executions refused or abandoned in `Shutdown::terminal_skipped`, and the transport logs which connections missed their handler. Idle connections close at drain start, which gives most handlers the full window.

**Detached tasks.** The drain tracks executions, not tasks. A background loop holding an `AppHandle`, a queue consumer or a poller, reads `handle.draining()` to stop pulling work when the transports stop accepting, and runs each unit of work through `execute` so the drain waits for it; `execute` answering `Closed` is far too late as a first notice.

Without `serve`, a job or CLI on `Connected`, `close` runs the same sequence. There are no sockets to close, and the drain covers live standalone executions: those whose `handle()` a detached subtask still holds.

`drain_timeout` and `shutdown_timeout` are set on the builder beside the `Timer` (§9.4), and every wait in the sequence is timed by it. `listen()` refuses an app that binds a transport without one, for two reasons: a served app with no `Timer` would run a zero-length drain and cut every in-flight request at every shutdown, undetectable until production, and it could not enforce the per-call deadlines it accepts. A `close` on `Connected` with no `Timer` runs a zero-length drain, cancelling and abandoning live executions at once, while its hooks run unbounded; §3.9 states the asymmetry.

---

## 10. Errors and diagnostics [44–47]

### 10.1 The wiring pass

`wire()` runs these steps and **collects** errors. It never stops at the first one.

1. **Module graph:** deduplicate identities, detect import cycles (printing the path of module names), check that re-exports are visible, refuse an `export::<T>()` of a key the module does not bind (`ExportNotBound`, with the hint to bind it there, or to re-export it with `reexport` when an import provides it), and refuse an input declared by a keyed module.
2. **Bindings:** find duplicate singles, single/collection mixes, aliases pointing at nothing, values whose `try_value` recorded an `Err` (naming the module and the key; `ModuleDef::try_value` and `Contribute::try_value` alike), a qualified contribution and a single binding under a role key the freeze recorded (§7; the hints "contribute it unqualified" and "contribute it"), and, in tests, overrides that match no binding or more than one, an `in_module::<M>()` over several instances of `M` among them, an override of `dyn Timer`, refused with the hint "set it with `TestApp::timer(..)`" (§3.9), a `replace_module` whose original no module imports (`ReplacementUnmatched`, the same stale mock §11 reports for an override), and a second `replace_module` of an original already replaced (`DuplicateReplacement { original, first, second }`, `first` and `second` the two `replace_module` calls, with the hint "keep one; wiring applied only the first"). Over an original nothing imports, the first call is `ReplacementUnmatched` and each later one `DuplicateReplacement`, never both on one call. An input a transport declared (`Transport::inputs`, transports/DESIGN §2.10) beside another transport's declaration of the key with a different seeder set, beside a module's declaration with another seeder, or beside a single binding under the key is `InputConflict { key, first, second }`, each side an `InputOrigin` (§10.2); two transports' declarations of one key whose seeder sets are equal, each written with `also_seeded_by`, merge into one; a binding is reported once per key, and inputs only modules declare keep `DuplicateBinding`.
3. **Visibility:** resolve every injection point against its module's visibility table. Report missing keys (with the injection point, the key and the module) and ambiguous keys (naming every source module). When a missing key's name equals a bound key's name up to a trailing `+ core::marker::Send + core::marker::Sync`, the report names both spellings: `dyn Repo` and `dyn Repo + Send + Sync` are distinct `TypeId`s, and this is the one place the mismatch is visible.
4. **Dependency cycles:** run a DFS over the resolved edges and print the full path, as in `A → B → C → A`, with the module of each step.
5. **Scopes:** run the needs-execution pass from §6.2, then report scope violations with the path that introduces the execution dependency, and hooks on types that became per-execution. Report a hook, readiness, module-hook or metadata closure that reads `Ext`, `ExecutionRef`, an input or a per-execution key, since no execution exists where it runs (§6.2). Then, for each handler, walk its reachable execution-scoped bindings and report every non-optional input that the handler's transport does not seed (§6.4), with the path from the handler to the service that reads the input, the handler's transport and the input's seeder.
6. **Environment:** check for a `Timer` wherever a wait is written: a readiness `.timeout`, `.attempt_timeout` or `.backoff`, a hook's or constructor's `After(..)`, or a builder knob set on an app with none (§3.9). `Default` and `Unbounded` need none. Each report points at the call that wrote the wait: `.backoff(..)` records its own location, so its report names that line and not the `.ready(..)` before it.

Steps that depend on a missing piece skip only the affected edges, so one missing binding doesn't hide unrelated errors.

Every type name in a report is a `TypeName` (§3.1), cut to its last path segment, inside generic arguments too: `Arc<PgPool>`, not `alloc::sync::Arc<my_app::db::PgPool>`. A generic transport marker prints the same way, `Q<X>`. Where one report would print two different types alike, `a::Config` and `b::Config`, it prints those types with their full paths, and only those: the report runs `TypeName::colliding` over every name it holds and writes each name in the answer with `{:#}`. A key enters the pass as its type and, when qualified, its qualifier, each on its own. `Store @ a::Replica` beside `Store @ b::Replica` prints `Store` short and the two `Replica`s in full, and `a::Config` beside `b::Config @ Q` prints both `Config`s in full; a key's binding kind plays no part. A transport enters the pass as the `TypeName` of its marker type wherever an entry stores one (§10.2): `InputNotSeeded`'s handler transport and seeder, `ClosureScopeViolation`'s transport, and the transports of `InputConflict`'s two origins. Module names follow the same rule under their own check: `billing::Module` and `users::Module` print in full where one report would print both. A `ModuleName` carries a label and an instance number a `TypeName` does not, which is why the two passes stay separate.

The pass is the report's, not the entry's. `WiringErrors` gathers the names of every entry before formatting, and a type colliding with one in another entry prints in full in both; a `WiringError` or an `InputOrigin` displayed on its own collides its own names. The startup report, `StartupError::Configure`, runs one pass over every failing transport and every type a `PrepareError` names, when the report is built (§10.2). The shutdown report runs one over its hook keys and its transports in `Display`. `StartupError::Bind` names one transport and runs none: a type cannot collide with itself.

What an entry stores decides what the pass reaches. A report entry stores the names it reports as values, a `TypeName`, a `Key` or a `KeyName`, rendered when the report is formatted under the rule above. Descriptive text, a consumer's name such as ``UserService (param `mailer`)``, the steps of a path between services or an item description, may be stored already rendered, with the names inside it written short: it is built while the graph is available and carries module names, field names and positions no `TypeName` holds. A name an entry also stores as a value is never written into its text: it is rendered from the value, and the entry prints it one way. `InputNotSeeded` holds its handler's transport as a value and its `steps` open after the handler, and the headline and the path both write the transport from that value; `ClosureScopeViolation`'s `closure` names the enhancer without its transport, which the report writes after it from the value. Two same-named services inside one dependency path still print alike, a step's type being inside pre-rendered text: that is the rule's scope as it stands.

A sample of the output:

```
error: wiring failed with 5 errors

  × missing dependency `dyn Mailer`
    ├─ needed by UserService (param `mailer`) in UsersModule
    └─ help: import a module that exports `dyn Mailer`, or provide it in UsersModule

  × missing dependency `dyn Repo`
    ├─ needed by ReportService (field `repo`) in ReportsModule
    └─ help: PersistenceModule exports `dyn Repo + Send + Sync`; the injection point reads `dyn Repo`

  × ambiguous dependency `dyn UserRepo` in AppModule
    ├─ exported by PersistenceModule   (src/persistence.rs:14)
    └─ exported by LegacyRepoModule    (src/legacy.rs:9)

  × scope violation: singleton `ReportService` depends on per-execution data
    └─ ReportService → AuditContext (execution) → Ext<CurrentUser>
       help: declare ReportService #[injectable(execution)], or inject a factory

  × input `RequestHead` is seeded by Http, read on a path from the Rpc handler UsersController::get_rpc
    ├─ UsersController::get_rpc (Rpc) → AuditContext (execution) → Dep<RequestHead> (field `head`)
    └─ help: read it as `Option<Dep<RequestHead>>` where the path is shared across transports
```

### 10.2 Error types

```rust
#[non_exhaustive]
pub enum StartupError {
    Wiring(WiringErrors),                                  // everything from wire()
    Connect(ConnectError),
    Configure(ConfigureErrors),                            // every failure the servers' `prepare` calls reported, before any binds (transports §2.7)
    Bind { transport: TypeName, source: Redacted },        // a transport's own error, or the core's `TimerMissing`; one name, printed short
}

/// Every `prepare` failure, one entry per server, collected before any server binds. `Display` writes every entry, and `Debug`
/// the same text. The entries' transports and the names of every `PrepareError` among them enter one `TypeName::colliding` pass
/// when the report is built, before each entry's text is redacted (§10.1).
pub struct ConfigureErrors { errors: Vec<ConfigureError> }

impl ConfigureErrors {
    pub fn iter(&self) -> slice::Iter<'_, ConfigureError>;
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
}

/// One server's configuration failure. `Display` writes the entry as its report does, the transport in full where the report's
/// pass decided so.
#[non_exhaustive]
pub struct ConfigureError { pub transport: TypeName, pub source: Redacted }

/// A `prepare` error whose text names types: what a server answers, boxed, so the startup report decides how each name prints.
/// `text` writes each name with `{:#}` when the set it receives holds it, and with `{}` otherwise; `Display` on a `PrepareError`
/// alone collides its own names.
pub struct PrepareError { names: Vec<TypeName>, text: Box<dyn Fn(&HashSet<TypeName>) -> String + Send + Sync> }

impl PrepareError {
    pub fn new(names: impl IntoIterator<Item = TypeName>, text: impl Fn(&HashSet<TypeName>) -> String + Send + Sync + 'static) -> Self;
    pub fn names(&self) -> &[TypeName];
    pub fn text(&self, full: &HashSet<TypeName>) -> String;   // each name in `full` written with its full path
}

/// `listen()`'s refusal of an app that binds a transport with no `Timer` (§9.5), stored in `StartupError::Bind`'s
/// `source` like a transport's error; `source.downcast_ref::<TimerMissing>()` tells it from a port already taken.
#[non_exhaustive]
pub struct TimerMissing { pub transport: TypeName }

/// A failure in the connect phase, on `StartupError::Connect` from `connect` and on `LoadError::Connect` from `load`.
#[non_exhaustive]
pub enum ConnectError {
    Construct { key: KeyName, module: ModuleName, reason: FailureReason },
    Readiness { key: KeyName, attempts: u32, reason: FailureReason },   // how the last attempt ended, or `TimedOut { limit: Item }` for the whole check (§9.3)
    Hook { hook: HookKind, key: KeyName, reason: FailureReason },
}

/// Why a hook, a construction, a readiness check or a transport's `close` did not complete. Held as a field on `ConnectError`,
/// `ShutdownFailure` and `LookupError::Construct`; implements `Display` and `Debug`, not `Error`, and nothing boxes one.
#[non_exhaustive]
pub enum FailureReason {
    Panicked(Redacted),                            // the payload, converted to a message
    TimedOut { after: Duration, limit: Limit },    // `after` is the configured duration of the limit that fired
    Skipped,                                       // never started: `shutdown_timeout` had expired (§9.5); shutdown only
    Errored(Redacted),                             // returned `Err`: a construction, an init or bootstrap hook, the last attempt of a readiness check,
                                                   // or a transport's `close`; or a closure hook whose parameter read failed, the `LookupError` by `downcast_ref` (§9.1)
}

/// Which limit a `TimedOut` hit.
#[non_exhaustive]
pub enum Limit {
    Item,          // an explicit bound on the whole item: a trait const's `After(..)`, or `.timeout(..)` on a binding handle (§3.9)
    Attempt,       // an explicit `.attempt_timeout(..)` on a readiness check (§9.3)
    Default,       // the app default for this kind of item: `hook_timeout` for a hook and for a transport's `close` under no cap, `construct_timeout` for a construction and for one attempt of a readiness check
    ShutdownCap,   // `shutdown_timeout` cutting a hook or a transport's `close` mid-run (§9.5)
}

/// An error from outside the core, after the redaction function (§9.3). Formatting writes the redacted text and never the
/// original; `source()` is `None`, so a reporter walking the chain stops at the text too. The original is reached through
/// the two methods alone.
pub struct Redacted { inner: BoxError, text: String }

impl Display for Redacted { /* writes `text` */ }
impl Debug   for Redacted { /* writes `text`; never formats `inner` */ }
impl Error   for Redacted { /* `source()` returns `None` */ }

impl Redacted {
    pub fn downcast_ref<E: Error + 'static>(&self) -> Option<&E>;   // the original, by type
    pub fn into_inner(self) -> BoxError;
}

/// The outcome of a shutdown, received by `serve` and by every `close` caller (§9.5).
#[non_exhaustive]
pub struct Shutdown {
    pub signal: Signal,            // the trigger that won
    pub abandoned: usize,          // executions still alive at the drain's end: `drain_timeout`, or the earlier `shutdown_timeout` expiry (§9.5)
    pub terminal_skipped: usize,   // terminal executions refused or abandoned (§9.5)
}

#[derive(Clone)]                                                  // one outcome, several receivers; the failures sit behind the Arc
#[non_exhaustive]
pub struct ShutdownError { pub report: Shutdown, pub failures: Arc<[ShutdownFailure]> }
// `Display` runs one `TypeName::colliding` pass over every failure's names, each hook key's type and qualifier and each
// `Close`'s transport, and writes each failure against it; a `ShutdownFailure` displayed alone collides its own names (§10.1).

#[non_exhaustive]
pub enum ShutdownFailure {
    Hook { hook: HookKind, key: KeyName, reason: FailureReason },   // `Errored` only for a closure hook whose parameter read failed: the hook traits return `()`
    Close { transport: TypeName, reason: FailureReason },          // `Errored` holding the transport's own error, `Panicked` for a `close` that panics, `TimedOut` for one exceeding its bound; never `Skipped` (§9.5)
}

/// `execute` and `Execution::open` from Draining on; `load` carries it in `LoadError::Closed` from Stopping on (§9.5).
#[non_exhaustive]
pub struct Closed;

/// Why `load` did not return a `ModuleRef` (§8.6).
#[non_exhaustive]
pub enum LoadError {
    Closed(Closed),
    Wiring(WiringErrors),      // the module wired against the frozen graph, every error collected
    Connect(ConnectError),     // a construction, readiness check or init hook of the module failed
    Refused(LoadRefusal),
}

/// What a lazily loaded module may not bring, because the graph has been handed out (§8.6).
#[non_exhaustive]
pub enum LoadRefusal {
    Controllers { module: ModuleName },
    Middleware { module: ModuleName },
    Contribution { module: ModuleName, key: KeyName },   // to a collection the module does not introduce
    GlobalExport { module: ModuleName, key: KeyName },
    Input { module: ModuleName, key: KeyName },          // inputs belong to transports; the per-handler check ran at wiring time (§6.4)
}

/// One wiring failure, a variant per failure of §10.1, held by `WiringErrors`. The two variants that name a transport hold it
/// as a `TypeName` and write it when the report is formatted; the entry's descriptive text carries no transport.
#[non_exhaustive]
pub enum WiringError {
    ..,
    /// Step 5: an enhancer a handler declares by closure in an explicit singleton scope, which reads per-execution data.
    /// `closure` names it without its transport, as in ``method-level guard #2 of UsersController::get``, and the report
    /// writes `transport` after it, as in `(Http)`; `role` is `guard`, `interceptor` or `error handler`; `path` runs from
    /// the injection point to the read of execution data.
    ClosureScopeViolation { closure: String, transport: TypeName, role: &'static str, path: Vec<String>, at: &'static Location<'static> },
    /// Step 5: a non-optional input read on a path from a handler whose transport does not seed it. `steps` holds the steps
    /// after the handler, each binding between it and the read and then the injection point; the report writes
    /// `{handler} ({transport})` ahead of them, the transport from the value, and names `seeder` in the headline.
    InputNotSeeded { handler: String, transport: TypeName, input: KeyName, seeder: TypeName, steps: Vec<String> },
    ..
}

/// One side of the `WiringErrors` entry `InputConflict { key, first, second }` (§10.1 step 2): where an input declaration,
/// or the binding it collides with, was written. `at` is the call's `#[track_caller]` location, as on every binding (§2).
/// `Display` writes the line the report prints, the transports short unless two different ones on the line print alike.
#[non_exhaustive]
pub enum InputOrigin {
    Transport { name: TypeName, at: &'static Location<'static> },                             // `Transport::inputs`; `name` is the marker type
    Module { module: ModuleName, seeders: Vec<TypeName>, at: &'static Location<'static> },    // `m.input::<T>().seeded_by::<Tr>()`, each `Tr`
    Binding { module: ModuleName, at: &'static Location<'static> },                           // a single binding under the key
}

#[non_exhaustive]
pub enum LookupError {                                           // [47]
    NotFound { key: KeyName, kind: LookupKind },                  // binding, extension, input
    NotReady { key: KeyName },                                    // a singleton the connect walk has not built yet (§8.5)
    WrongType { key: KeyName, requested: &'static str },          // an erased `Key` asked for a `T` it does not hold
    WrongKind { key: KeyName, expected: BindingKind, found: BindingKind },   // single read as collection, or the reverse
    ExecutionRequired { key: KeyName },
    Ambiguous { key: KeyName, sources: Vec<ModuleName> },         // a runtime lookup of a key two visible exports answer (§8.2)
    AmbiguousModule { module: &'static str, candidates: Vec<ModuleName> },   // `app.module::<M>()` over several instances of `M` (§8.5)
    Construct { key: KeyName, reason: FailureReason },            // a build inside a call failed, panicked or timed out (§3.4)
    Closed { key: KeyName },                                      // a singleton lookup from Destroying on (§9.5)
}

/// What `Construct::construct` fails with (§3.4). `Dependency` is a dependency's own `LookupError`, reported unchanged;
/// `Failed` is the constructor's own error, redacted and reported as `FailureReason::Errored`. A panic or a timeout is
/// no variant: a constructor cannot report either about itself, and the core records both from outside the poll.
#[non_exhaustive]
pub enum ConstructError {
    Dependency(LookupError),   // `?` on a dependency read, through `From`
    Failed(BoxError),    // `ConstructError::failed(e)`, or the macro's mapping of a constructor's `Result<Self, E>`
}

impl From<LookupError> for ConstructError { /* `Dependency` */ }   // no blanket `From<E: Error>`: it would overlap this impl (§3.4)

impl ConstructError {
    pub fn failed(e: impl Into<BoxError>) -> Self;   // `Failed`
}

/// A guard's `Ok(false)`, as the error handlers see it (§7).
#[non_exhaustive]
pub struct GuardRejected { pub guard: &'static str }

/// A panic in one stage of the pipeline, caught at that stage (§7): the interceptors around a panicking handler receive it
/// from `next.run()`, and the error handlers receive any stage's.
#[non_exhaustive]
pub struct PanicRecovered { pub stage: DispatchStage, pub message: Redacted }   // the payload, converted to a message

/// Which stage of `dispatch` panicked.
#[non_exhaustive]
pub enum DispatchStage { Guard, Interceptor, Handler, ErrorHandler }

/// `true` wherever a core error carries a caught panic: a `PanicRecovered`, a `LookupError::Construct { reason: Panicked(..) }`,
/// the two shapes a panic during a call takes (§7), and a `ConnectError` whose reason is `Panicked`, bare or inside
/// `LoadError::Connect` or `StartupError::Connect`, which a handler propagating a `load` inside a call hands over (§8.6).
/// It follows a wrapper's chain to 32 levels: `Errored`'s `Redacted`, `ConstructError::Failed` and every `source()`.
/// Inside a `Redacted` only the core's own types are looked through (below).
pub fn is_panic(error: &BoxError) -> bool;
```

`is_panic` takes `&BoxError` because that is what a caller holds: an error handler receives `err: BoxError`, an interceptor gets one from `next.run()`, and `is_panic(&err)` compiles against both. Against a `dyn` parameter, `&(dyn Error + 'static)` or `&(dyn Error + Send + Sync + 'static)` alike, `is_panic(&err)` is E0277: the coercion commits to unsizing the `Box` itself, and `Box<dyn Error>` does not implement `Error`; the caller would have to write `&*err`. The cost is that a caller holding a bare `&dyn Error`, a `source()` link, cannot pass it; `is_panic` walks `source()` itself. Nesting is rare: a dependency's `LookupError` passes up unchanged through `ConstructError::Dependency`, so a panic any depth down a dependency chain arrives as the top-level `LookupError::Construct { reason: Panicked(..) }` naming the deepest key. The nesting that does occur is a constructor or a `try_` factory returning a dependency's `LookupError` as its own `Err`, which arrives as `Construct { reason: Errored(Redacted(LookupError::Construct { reason: Panicked(..) })) }`, and `is_panic` reaches it. Its limit follows from `Redacted` handing out its original by type alone: inside one, only `PanicRecovered`, `LookupError`, `ConstructError`, `ConnectError`, `LoadError`, `StartupError` and `Redacted` are recognised, and a user's error type wrapping a panicked `LookupError`, returned as a constructor's `Err` and redacted, is not looked through. Reaching it would need a `&dyn Error` accessor on `Redacted`, a third path to the original, which this section rules out.

`StartupError::Configure` is the startup report, one `ConfigureError` per server whose `prepare` failed (transports §2.7). A server whose failure names types answers a `PrepareError`, boxed, as its `prepare` error: the names, and a closure writing the text given the set of names to write in full. `listen()` holds every server's error before it builds the report. It downcasts each to `PrepareError`, runs `TypeName::colliding` once over every failing transport together with the names of every `PrepareError`, and writes each `PrepareError`'s text against that set. The text is then redacted against the graph's secrets and stored as the entry's `source`, which is why the pass runs when the report is built and not in `Display`, where no registry exists. A server answering any other error keeps its text, which contributes no names, and its transport still enters the pass. Each `ConfigureError` records whether the pass wrote its transport in full, and an entry displayed alone prints as the report does; `source.downcast_ref::<PrepareError>()` reaches the original, whose own `Display` collides its own names. `ShutdownError` holds no text written ahead of time, and its reasons are redacted when recorded, so its pass runs in `Display`.

`WiringErrors` carries one entry per failure from §10.1, the `Err` a `try_value` recorded among them, held as a `Redacted` (§9.3). `WrongType` is the one failure a caller reaches deliberately, through `Resolver::by_key::<T>(key)` (§3.1), where an erased key does not fix `T`. A typed read answers it too, `dep`, `many`, an entry's `resolve` or an input read, when the stored instance does not hold the key's type after the recorded coercion; with consistent records that cannot happen, and the alternative to the variant is a panic.

Every public error type is `#[non_exhaustive]`, the seven structs included: a variant added to an enum breaks no caller that matches on it, a field added to `Shutdown`, `ShutdownError`, `ConfigureError`, `GuardRejected`, `PanicRecovered` or `TimerMissing` breaks no caller that destructures one, and `Closed` is constructed by the core alone. `Redacted`, `ConfigureErrors` and `PrepareError` carry no attribute: their fields are private, which closes them the same way. Code outside the core reads these types and builds none of them but `ConstructError`, which a constructor returns, and `PrepareError`, which a server's `prepare` returns. `ConnectError` is its own type because `StartupError` and `LoadError` share only the connect phase. `load` reports wiring errors through its own `Wiring` and binds no transport; a `LoadError` wrapping a whole `StartupError` would carry two cases that can be constructed and never occur. Composed from exact parts, every variant of both is reachable, and code handling a construction failure handles it once for startup and load alike.

Neither the core nor the macros panic or exit [45]. Panics inside user constructors, factories, readiness checks and hooks are caught at the poll boundary and reported as `FailureReason::Panicked` on `ConnectError::Construct`, `ConnectError::Readiness`, `ConnectError::Hook` or `ShutdownFailure::Hook`; a transport's `close` is caught the same way and reported on `ShutdownFailure::Close` (§9.5). A panic inside a call's pipeline, in a guard, an interceptor, the handler or an error handler, is caught at that stage and travels as `PanicRecovered`, through the interceptors around a handler and then to the error handlers (§7); one while the container builds an enhancer or the controller inside a call is `LookupError::Construct { reason: Panicked(..) }`, and `is_panic` recognises both, and a `ConnectError` carrying one. That holds unless the binary is built with `panic = "abort"`, where nothing can be caught. `FailureReason` is one enum for every place a hook, a construction, a readiness check or a transport's `close` can fail, so a timeout reads the same on an init hook, a destroy hook, a constructor, a check and a `close`; `Skipped` is reachable at shutdown alone, on `Hook` alone, nothing capping startup as a whole and every `close` starting. It is a field everywhere it appears and implements no `Error`: nothing boxes a reason, and `BoxError::from(reason)` does not compile. `TimedOut` names the limit that fired and carries that limit's configured duration: an item's own bound and the cap are often the same round number, and the duration alone would not tell them apart. A hook or a `close` still running when the cap expires reports `ShutdownCap`, whatever its own bound was.

`Readiness` keeps `attempts`, and its `reason` is how the check ended, by the rules in §9.3. `Limit::Default` has one meaning, the app default for the item's kind, and `after` carries that default's duration: `construct_timeout` on a check's attempt or a construction, `hook_timeout` on a hook.

The core consumes a `ConstructError` by variant. Inside a call, a `Dependency` is reported as the `LookupError` it carries, on the path that error was already taking. During `connect`, where `ConnectError` carries no `LookupError`, a `Dependency` holding a nested build's `Construct { key, reason }` becomes `ConnectError::Construct` naming that deeper key, and one holding any other lookup error, the `NotReady` a constructor's `ModuleRef::get` meets, becomes `ConnectError::Construct` naming the binding itself with `reason: Errored(..)`, the `LookupError` by `downcast_ref`. A `Failed` is reported as `Construct { reason: Errored(..) }`, on `ConnectError` during `connect` and on `LookupError` inside a call. `Failed` holds the error as the constructor returned it, and the redaction function (§9.3) runs when the core stores it as `Errored`. It runs on `Failed` alone: a `LookupError` is the core's own, and any outside error inside it was redacted where it was stored.

Every error the core did not create itself, from user code, integrations or transports, passes through the redaction function before it is stored in any core error type (§9.3), and the field that stores it is a `Redacted`: `FailureReason::{Errored, Panicked}`, a transport's `close` error among the former, the `source` of `StartupError::Bind`, the `source` of each `ConfigureError`, and the `try_value` entry in `WiringErrors`. A bind error rarely carries a credential, but a TLS key path or a proxy URL with a password in it can, and the cost is one call on a path that fails once. `Bind` also carries the one error the core writes itself on that path, `listen()`'s refusal of an app with a transport and no `Timer` (§9.5): a `TimerMissing { transport }`, wrapped in the same `Redacted` to keep the field one type, and a struct rather than a message, which lets `downcast_ref::<TimerMissing>()` tell a misconfigured app from a port already taken. The type is kept because the original has to stay reachable on the runtime path: `LookupError::Construct` fires inside a call, and an execution-scoped constructor failing with a domain error, a tenant not found, is mapped to a 404 by an error handler that downcasts the `LookupError`, then the `reason`'s `Redacted` to the domain type. With the original replaced by its text that mapping would be impossible. What the type enforces is that no formatting prints the original: `Display` and `Debug` write the text, and `source()` is `None`, which is what keeps an error-chain reporter, one that walks `source()` and prints every link, from printing the original and bypassing the redaction. The original is reached through `downcast_ref` and `into_inner` and nowhere else, two methods a reviewer can find.

`Closed` is a refusal by phase (§9.5): `execute` and `Execution::open` answer it from Draining on, `load` from Stopping on inside `LoadError`, which holds `load`'s other failures beside it; `StartupError` never carries `Closed`. The lookup form is `LookupError::Closed`, from the first destroy hook on, so an abandoned execution is never handed an instance whose destroy hook has run; one it already holds stays a valid object.

### 10.3 Compile-time diagnostics [46]

Compile-time diagnostics use two mechanisms:
- **Trait-bound errors** carry `#[diagnostic::on_unimplemented]` messages written as instructions ("write `Dep<PgPool>`", "implement `Guard<Http>`", "remove the scope argument").
- **Macro errors** use `syn::Error` spans on the exact token (two `#[construct]` functions, no constructor and no `new`, a provider in `#[module]` that doesn't implement `Construct` together with the hint "add #[injectable] or bind it with a factory").

The macro emits one assertion per injection point with `quote_spanned!`, so errors point at the offending field or parameter rather than at the macro invocation.

---

## 11. Testing [48]

```rust
let app = TestApp::of(AppModule)
    .override_value::<dyn UserRepo>(Arc::new(InMemoryRepo::default()))
    .override_factory(|| async { FixedClock::at(t0) })
    .override_try_factory(|| async { TestBus::connect().await })
    .override_value::<AuditLog>(Arc::new(NullAudit)).in_module::<BillingModule>()
    .override_value::<PgPool>(Arc::new(replica_pool)).in_module_keyed::<DbModule, Replica>()
    .replace_module(MailModule, FakeMailModule)
    .connect()
    .await?;

let users = app.get::<UserService>().await?;
```

Override rules:
- An override replaces the recipe of an existing key and keeps its origin module, its visibility and its exports, so production modules stay untouched.
- An override names no module by default and must match exactly one binding. One that matches none is a wiring error, which catches stale mocks after refactors. One that matches several, two modules each binding the key privately, is a wiring error listing every match; `.everywhere()` replaces all of them explicitly. Replacing several by default would make a test pass for the wrong reason.
- `.in_module::<M>()` scopes an override to one module. Over several instances of `M`, configured or keyed, it is itself ambiguous and fails as one; `.in_module_keyed::<M, Q>()` and `.in_module_of(&config)` name one instance.
- An override targets `T @ ()`. A binding written `.qualified::<Q>()` is reached by the same method on the pending override, `.override_value::<PgPool>(fake).qualified::<Replica>().in_module::<DbModule>()`, one spelling for every override kind, in the order the binding it replaces was written. `TestApp` carries a state: `Settled` between overrides, `Pending` after a single override, where `.qualified`, `.in_module*` and `.everywhere()` describe it. `.qualified` answers `TestApp<Pending>`, so `.in_module*` and `.everywhere()` still follow, and a second `.qualified` replaces the first. Inside a keyed module the binding's own key is unqualified, and `in_module_keyed::<M, Q>()` reaches it as written.
- An override can't change a key's kind (single or collection). `override_many` replaces an entire collection and answers `TestApp<PendingMany>`: its `.qualified::<Q>()` targets the collection `T @ Q` and answers `TestApp<Settled>`, and it has no `.in_module*`, a collection being app-wide (§8.2). Every method the states share stays reachable: `override_many(..).connect()` reads as before.
- `override_factory` and `override_try_factory` follow §4's split: the plain one binds the future's output, the `try_` one its `Ok` type.
- `replace_module` swaps by identity. The replacement must export a superset of the original's keys, or wiring reports what's missing. One whose original no module imports is a wiring error, `ReplacementUnmatched`, the same stale mock an override that matches nothing is. A second `replace_module` of an original already replaced is a wiring error, `DuplicateReplacement`, naming both calls.
- The `Timer` is app configuration, not a binding to override. `TestApp::timer(..)` sets the one clock that times the drain, the hooks, the constructions and the readiness checks and that services read as `Dep<dyn Timer>` (§3.9); `override_value::<dyn Timer>` is a wiring error with the hint "set it with `TestApp::timer(..)`".

---

## 12. What is refused, and where

| Refusal | When | Mechanism |
|---|---|---|
| A field or parameter whose type is not `FromContainer` | compile | `FromContainer` bound + `on_unimplemented` |
| `Dep<T>` with `T` not `Send + Sync` | compile | bound on `Dep`; the auto trait's own message, no hint |
| `Ext` / `ExecutionRef` in an explicit singleton | compile | `AllowedIn<S>` |
| A hook on an explicit execution-scoped or transient type | compile | `Construct<Scope: HookCapable>` |
| A closure hook or `.ready(..)` on an execution-scoped or transient handle | compile | typed binding handle (§9.1) |
| A controller-level `value = expr` | compile | macro span error, "declare it per method, or bind it by type for shared state" (§7) |
| `.qualified::<Q>()` on an enhancer contribution | compile | `Contribute<.., Enhancer>` has no `qualified`; a role collection is read unqualified (§7) |
| `enhancer::<K>()` with a `K` that is not a role key | compile | `Role` bound + `on_unimplemented` (§3.7) |
| A second bound on one binding-handle item: `.timeout` twice, `.unbounded()` beside `.timeout` or `.attempt_timeout`, either after `.unbounded()` | compile | typed binding handle: the item's bounded state has neither method (§9.1) |
| An enhancer that lacks its role trait, including a controller-level one for any handler's transport | compile | `Guard<T>` and similar bounds, one per handler, naming the handler |
| A constructor that isn't `Send` | compile | generated impl |
| Two or zero constructors, a bad attribute, a malformed `into` item | compile | macro span error; the `into` text names the three forms (§4) |
| `X as <role key>` in a `providers` list | compile | macro span error on the role key (§4); reads the key as written, so an alias passes to the wiring error below |
| Trait binding with a non-implementing type | compile | coercion in the closure |
| `?` on a value whose expression failed | startup (`wire`) | `try_value` record, `ModuleDef`'s for a `providers` entry and `Contribute`'s for an `into` entry (§4) |
| Missing dependency | startup (`wire`) | visibility pass |
| `dyn Repo` bound as `dyn Repo + Send + Sync`, or the reverse | startup (`wire`) | missing-dependency report naming both spellings |
| Two sources for one key | startup (`wire`) | visibility pass, naming both |
| Duplicate single binding, single/collection mix | startup (`wire`) | binding pass |
| A qualified contribution under a role key the freeze recorded, `contribute::<AnyGuard<Http>>().qualified::<Q>()` | startup (`wire`) | freeze pass, roles by `TypeId` (§7); hint "contribute it unqualified" |
| A single binding under a role key the freeze recorded: `also_as`, an alias, `provide` or `value` | startup (`wire`) | freeze pass (§7); hint "contribute it" |
| Dependency cycle, module import cycle | startup (`wire`) | DFS with path |
| An `export::<T>()` of a key the module does not bind | startup (`wire`) | module graph pass; `ExportNotBound`, with the `reexport` hint when an import provides it |
| `ModuleDef::controller::<C>().at(prefix)` on a controller none of whose handlers' transports read a prefix, or that mounts no handler | startup (`wire`) | freeze pass against `Transport::READS_PREFIX` (`transports/DESIGN.md` §2.5), naming the controller, the prefix, the transports or the absence of handlers, and the `.at` call's location |
| Singleton → execution dependency, including through transients: an explicit singleton, `#[injectable(singleton)]`, `with(singleton) = ..` or `.singleton(..)`, reading execution data (§3.3) | startup (`wire`) | needs-execution pass; `ScopeViolation`, or `ClosureScopeViolation` for an enhancer a handler declares by closure, its transport held as a `TypeName` (§10.2) |
| An `Auto` provider reading execution data, a type or a `with` closure under a provider key alike; the hint names `#[injectable(execution)]` for a type and `with(execution)` or `.execution(..)` for a closure (§6.2) | startup (`wire`) | needs-execution pass |
| A hook on an `Auto` type inferred per-execution, a closure hook or `.ready(..)` included | startup (`wire`) | scope pass |
| A hook, readiness, module-hook or metadata closure reading `Ext`, `ExecutionRef`, an input or a per-execution key | startup (`wire`) | scope pass (§6.2) |
| A non-optional input read on a path from a transport that doesn't seed it | startup (`wire`) | per-handler input check; `InputNotSeeded`, the handler's transport and the seeder held as `TypeName`s, the steps after the handler as text (§10.2) |
| An input declared by a keyed module | startup (`wire`) | module graph pass |
| One input declared by two transports, or a transport's declaration beside a module's with another seeder or beside a binding under the key | startup (`wire`) | binding pass; `InputConflict`, each side an `InputOrigin` |
| An override that matches nothing, or more than one binding, `in_module::<M>()` over several instances of `M` included | startup (`wire`) | test builder |
| A `replace_module` whose original no module imports | startup (`wire`) | test builder; `ReplacementUnmatched` |
| A second `replace_module` of an original already replaced | startup (`wire`) | test builder; `DuplicateReplacement`, naming both calls |
| `override_value::<dyn Timer>` | startup (`wire`) | test builder; hint "set it with `TestApp::timer(..)`" (§3.9) |
| A module binding `dyn Timer` itself | startup (`wire`) | two sources for one key, against the core's global export |
| A written wait, `After(..)`, `.timeout`, `.attempt_timeout`, `.backoff` or a builder knob, on an app with no `Timer` | startup (`wire`) | environment pass (§3.9), reported at the call that wrote the wait |
| A constructor, factory, readiness check or hook fails, panics or times out | startup (`connect`) | `StartupError::Connect`, the case named in `ConnectError` and the reason on `FailureReason` |
| `ModuleRef::get` for a singleton not yet built | startup (`connect`) | `LookupError::NotReady` |
| A transport bound on an app with no `Timer` | startup (`listen`) | `StartupError::Bind` carrying `TimerMissing { transport }` in its `source`, reached by `downcast_ref` (§10.2); the drain is timed by the `Timer` (§9.5) |
| A lazy module with controllers or middleware, contributions to a collection it doesn't introduce, global exports, or an execution input | runtime (`load`) | `LoadError::Refused`, the case named in `LoadRefusal` (§8.6) |
| A lazy module that fails to wire, or whose construction, readiness check or init hook fails | runtime (`load`) | `LoadError::Wiring`, `LoadError::Connect` |
| `load` from Stopping on | runtime (`load`) | `LoadError::Closed` (§9.5) |
| `execute` or `Execution::open` from Draining on | runtime | `Closed`; `open_terminal` with a `DrainToken` is allowed in Draining (§6.3) |
| A singleton lookup from Destroying on | runtime | `LookupError::Closed` |
| Lookup not found, not ready, wrong type, wrong kind, no execution, ambiguous key, ambiguous module, a build inside a call that fails or times out | runtime | `LookupError` |
| An execution input not seeded by a standalone execution, an extension not written | runtime | `LookupError::NotFound`, or `None` through `Option` |
| A guard's refusal | runtime | `GuardRejected` through the error handlers |
| A panic in a guard, interceptor, handler or error handler | runtime | caught at its stage; `PanicRecovered` through the interceptors around a handler and the error handlers (§7); `is_panic(&BoxError)` recognises it, a `LookupError::Construct { reason: Panicked(..) }` and a `ConnectError` carrying one, through wrapping `Err`s and `source()` chains (§10.2) |
| A shutdown hook panics, times out, is skipped by `shutdown_timeout` or fails to read a parameter, or a transport's `close` fails, panics or exceeds its bound | runtime (`close`) | `ShutdownFailure` inside `ShutdownError`, `Hook` and `Close` each with a `FailureReason`; the later steps still run |

---

## 13. The integration crate surface (no macros)

This is a complete database integration written against the public API:

```rust
use fw_core::{
    scope, BoxError, Construct, ConstructError, Dep, Dependencies, Hooks, Module, ModuleDef, ModuleIdentity, OnModuleInit,
    Resolver, Secret,
};

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

        m.try_singleton(move || {                    // `connect_lazy` returns a `Result`; the `Ok` type is the key (§4)
            let url = url.clone();
            async move { PgPoolOptions::new().max_connections(max).connect_lazy(url.expose()) }
        })
        .ready(|pool: Dep<PgPool>| async move { sqlx::query("select 1").execute(&*pool).await.map(drop) })
        .retries(5)
        .timeout(Duration::from_secs(10))
        .on_destroy(|pool: Dep<PgPool>| async move { pool.close().await });

        m.provide::<Migrations>();                   // a `Construct` type: its hooks run (§9.1)

        m.contribute::<dyn HealthIndicator>()
            .singleton(|pool: Dep<PgPool>| async move { PgHealth::new(pool) }, |a| a);

        m.export::<PgPool>();
    }
}

pub struct Migrations { pool: Dep<PgPool>, set: Migrator }

impl Construct for Migrations {
    type Scope = scope::Singleton;

    fn dependencies(d: &mut Dependencies) { d.field::<Dep<PgPool>>("pool"); }

    async fn construct(r: &Resolver<'_>) -> Result<Self, ConstructError> {
        let pool = r.dep::<PgPool>().await?;                                 // a `LookupError` passes through as `Dependency`
        let set = Migrator::new(Path::new("./migrations")).await.map_err(ConstructError::failed)?;
        Ok(Self { pool, set })
    }

    fn hooks(h: &mut Hooks<Self>) { h.on_module_init(); }
}

impl OnModuleInit for Migrations {
    async fn on_module_init(&self) -> Result<(), BoxError> { Ok(self.set.run(&*self.pool).await?) }
}
```

The value API an integration writes against:

| Area | API |
|---|---|
| Modules | `Module`, `ModuleIdentity`, `ModuleDef::{import, global, export, reexport, secret, on_init, on_destroy, meta, enhancer}`, `DynamicModule`, `Keyed` |
| Bindings | `provide::<T: Construct>`, `provide_with::<T>(factory)`/`try_provide_with`, `value`/`try_value`, `with`/`try_with` (`Auto`, §3.3), `singleton`/`try_singleton`, `execution`/`try_execution`, `transient`/`try_transient`, `contribute::<T>()`, `enhancer::<R: Role>()` (the same builder marked `Enhancer`, for a role key of a transport with no mounted handler), `Contribute<'m, U, Q, Mark = Plain>` (`provide`, `value`, `try_value(Result<Arc<U>, E>)`, `singleton`/`try_singleton`, `execution`/`try_execution`, `with`/`try_with` (`Auto`, §3.3), each with the coercion closure; `qualified` on `Plain` alone), `handle::{Plain, Enhancer}` (empty enums, the marks), `alias::<T, Q>().of::<Existing>()`, `input::<T>().seeded_by::<Tr>()` |
| Binding handles | `also_as`, `qualified::<Q>`, `timeout(..)`/`unbounded()` on the binding itself, `ready(..).retries(..).backoff(..).attempt_timeout(..).timeout(..)`/`.unbounded()`, `on_init`/`on_destroy`/`before_shutdown`/`on_shutdown` each with `.timeout(..)`/`.unbounded()` (singleton and `Auto` handles); every bound written once per item (§9.1) |
| Injection points | `FromContainer`, `Requirement`, `Dependencies::{field, param, add}`, `Key`, `KeyName`, `TypeName`, `Resolver::{dep, dep_qualified, many, many_qualified, entries, ext, input, module, execution, by_key}` |
| Construction | `Construct` (with `CONSTRUCT_TIMEOUT`), `Hooks<T>::{on_module_init, on_application_bootstrap, on_module_destroy, before_application_shutdown, on_application_shutdown}`, `hooks!`, `ConstructError` |
| Transports | `Transport`, `Role`, `Controller::mount`, `Mount` (whose `handler` records the transport's role keys for the freeze), `dispatch`, `Execution::{open, open_terminal, seed, handle}`, `DrainToken`, `ExecutionRef`, `EnhancerSpec` (`guard_with`/`interceptor_with`/`error_handler_with`, `Auto`; `guard_with_in::<S>` and kin take the scope, §3.3), the `Erased*` role twins |
| Runtime | `Timer`, `Bound`, `Signal`, `Server` (implemented by transports for `prepare`, `bind`, `bound`, the drain, which hands over the `DrainToken`, and `close`), `AppHandle`, `Cancelled`, `Draining`, `Shutdown`, `Closed` |
| Errors | `StartupError`, `TimerMissing`, `ConnectError`, `FailureReason`, `Limit`, `WiringErrors`, `WiringError`, `ConfigureErrors`, `ConfigureError`, `PrepareError`, `LoadError`, `LoadRefusal`, `ShutdownError`, `ShutdownFailure`, `LookupError`, `GuardRejected`, `PanicRecovered`, `DispatchStage`, `InputOrigin`, `is_panic`, `Redacted`, `Secret` |

The transport layer extends this surface, and `transports/DESIGN.md` §11 states each extension where a requirement forces it: `Transport::KEY` and `Transport::inputs`, whose declarations an `InputConflict` reports through `InputOrigin` (§10.2); `Transport::READS_PREFIX`, by which the freeze refuses `ModuleDef::controller::<C>().at(prefix)` on a controller none of whose handlers' transports read a prefix, or that mounts none; a `HandlerSpec` builder for `Mount::handler` carrying metadata, route, shape and the handler's own dependencies, with `Execution::handler()`, `AppHandle::handlers()` and `ModuleDef::controller::<C>().at(prefix)`; `Mount::once`; impl-level shared values through `EnhancerSpec::{guard,interceptor,error_handler}_arc`; `CancelReason`, `cancel_with` on `Execution` and `ExecutionRef`, `cancel_reason` and `on_stream_end`; `Server::prepare` before any bind, `Server::bound` and `StartupError::Configure(ConfigureErrors)`, a fourth variant shaped as `Wiring(WiringErrors)` is, with `PrepareError` as what a `prepare` answers when its text names types (§10.2); `dispatch_late` and `is_late` for errors after a stream has started; `Execution::route_to`; `Mounted::module_meta` and `ModuleRef::with_execution`; `Mounted::handlers_reading` and its `InputReader` (`handler()`, `transport()`, `steps()`), built on the input walk (§6.4); `AppHandle::phase`, `AppHandle::redact` and `AppHandle::drain_timeout`; `Inputs::also_seeded_by`, with the freeze merging equal seeder sets (§6.4); `AppHandle::mounted::<T>()` and `Mounted::handlers_of::<U>()`, the mounted handlers of a transport read from a handle or from another transport's server; and `TransportMetadata`, by which a metadata type names the one transport it is valid on, read by a probe the handler attributes emit. Nothing above changes for them: each is an addition at the point the transport design names.

---

## 14. Decisions taken with defaults (open to change)

1. **Collection and extension spelling.** `Dep<T>` stays the single-dependency spelling. Collections are `Many<T>` and execution views are `Ext<T>`, because they read differently and fail differently.
2. **The qualifier is a second type parameter:** `Dep<PgPool, Replica>`, defaulting to `()`.
3. **Collections are app-wide**, not limited by visibility.
4. **Module structure is synchronous.** Registration can't await. Anything that needs async configuration becomes an async factory. This keeps "no I/O before wiring completes" true.
5. **Panics in user code are caught** at poll boundaries and reported as `FailureReason::Panicked` on the error of the phase: `ConnectError` during `connect` and `load`, `ShutdownFailure` during `close`, a transport's `close` included, `LookupError::Construct` inside a call. In a call's pipeline, each stage runs under its own catch and a panic becomes `PanicRecovered { stage, message }` where it happened: a handler's passes back through the interceptors as the `Err` of `next.run()`, and every stage's reaches the error handlers, so four transports do not each write the catch. `is_panic(&BoxError)` recognises it, the `LookupError::Construct { reason: Panicked(..) }` of a container build inside a call, and a `ConnectError` carrying one, bare or inside `LoadError` or `StartupError`; it follows a constructor's or `try_` factory's wrapping `Err`, `ConstructError::Failed` and `source()` chains to 32 levels, and inside a `Redacted` looks through the core's own types alone. It takes `&BoxError`: `is_panic(&err)` on the `BoxError` a handler holds compiles against that and is E0277 against `&dyn Error`.
6. **Singletons are eager.** They're built during `connect`, never lazily on first use.
7. **Auto scope** is singleton for providers and inferred for controllers and enhancers. An explicit `singleton` opts out of inference.
8. **Collection order** is depth-first post-order over imports from the root, imports in the order written, then declaration order inside a module. Hooks and readiness checks tie-break on the same order.
9. **The transport owns the execution**, or `execute` does for a standalone one; everything else holds a clone. Cancellation fires while clones are alive: the transport fires it for a call at a disconnect or the deadline, a standalone execution's fires at its deadline, and the drain's end fires it on whatever is still alive, never on `execute` completing. The execution ends, and its instances drop, when the last clone does.
10. **`Many<T>` is eager.** Reading it constructs every contribution. The lazy form, `Resolver::entries::<T>()`, is a transport's surface, not an injection point.
11. **Shutdown is one event.** The first trigger wins, `serve`'s signal or a `close` on any handle, and its signal reaches the hooks; a later `close` joins the running shutdown and ignores its own signal. Every receiver gets the one outcome, which is why `ShutdownError` is `Clone`.
12. **The drain comes before cancellation.** In-flight executions get `drain_timeout`, ten seconds unset and timed by the `Timer`, to end on their own; `draining()` is the notice at its start and cancellation the deadline signal at its end, `drain_timeout` or the earlier expiry of `shutdown_timeout`. Those still alive are abandoned and counted in `Shutdown::abandoned`. Without a `Timer` the drain is zero-length.
13. **`BeforeApplicationShutdown` runs while the app still serves.** The order is before-shutdown, stop accepting, drain, destroy, sockets, shutdown. `load` is refused from Stopping on with `LoadError::Closed` and `execute` from Draining on with `Closed`; a singleton lookup from the first destroy hook on is `LookupError::Closed`.
14. **Every bound is a `Bound`.** A hook defaults to `hook_timeout` (10 s); a construction, and one attempt of a readiness check, to `construct_timeout` (30 s). Every bounded item is in one of the same three states, default, explicit or unbounded, written where it is declared and written once: after `.timeout(..)` or `.unbounded()` the handle's state has neither method, and a readiness check writes `.timeout` and `.attempt_timeout` once each or `.unbounded()` in place of both. The defaults apply only with a `Timer`, an explicit bound without one is a wiring error, and `Unbounded` needs none. `shutdown_timeout` caps the whole sequence from the trigger and is unset by default.
15. **A timed-out hook is dropped and reports which limit fired, the item's own, one attempt's, the app default or the cap, with that limit's duration; a timed-out execution is cancelled and left running.** The first stops at its next await, the second is still held by its transport or subtask.
16. **Disconnect handlers at shutdown are best effort.** Terminal executions run inside the drain window and no grace window follows it; `Shutdown::terminal_skipped` counts the losses, and state that must survive a crash lives in TTL or heartbeat storage.
17. **Redaction is scoped by origin and stored as a type.** Every error the core did not create itself, from user code, integrations or transports, passes through the redaction function before it is stored in any core error type; the scope is the origin, with no list of variants to maintain. What is stored is a `Redacted`: `Display` and `Debug` write the text, `source()` is `None`, and the original is reached through `downcast_ref` and `into_inner` alone, which is what lets an error handler map a constructor's domain error to a status.
18. **A role is decided by `TypeId` at freeze.** Mounting a handler records its transport's three role keys, and every contribution under one of them is an enhancer however it was registered, so an alias or a `macro_rules` wrapper changes nothing and no type name is read. `m.enhancer::<R: Role>()` marks a contribution explicitly for a role key of a transport with no mounted handler, bounded by the sealed `Role` trait the three role keys implement, and returns the `contribute` builder marked `Enhancer`, on which `qualified` does not exist; one marked contribution marks the whole key. The freeze refuses two forms under a recorded role key, each of which nothing reads: a qualified contribution, with the hint "contribute it unqualified", and a single binding, through `also_as`, an alias, `provide` or `value`, with the hint "contribute it". A role key no mounted handler reads and no `enhancer` marks is not checked. Every `into` list in `#[module]` lowers to `contribute` and takes a type, `value = expr`, `value = expr?` or `with = closure`, the closure `Auto` unless it writes `with(scope)` (item 22). No `#[module]` spelling marks a role key of a transport with no mounted handler; that is a hand-written `m.enhancer::<K>()`, anywhere in the graph.
19. **Diagnostics print short type names, and full paths on collision.** Every type name in a report is a `TypeName`, cut to its last path segment, inside generic arguments too. Where one report would print two different types alike, a key's type or qualifier or a transport's marker, those print with their full paths; module types follow the same rule under their own check. An entry stores the names it reports as values and renders them when the report is formatted. Descriptive text may be stored rendered, its names short, and never carries a name the entry also stores as a value (§10.1).
20. **A transport's `close` is bounded.** Each runs under what is left of `shutdown_timeout`, or under `hook_timeout` when no cap is set, and one that exceeds its bound is dropped and recorded as `ShutdownFailure::Close { transport, reason }`, the shape `Hook` has, `reason` a `TimedOut` naming `ShutdownCap` or `Default`, an `Errored` for the transport's error and a `Panicked` for a panic, caught as a hook's is; a `close` can wait on a TLS close-notify or a broker's acknowledgment, which is what the cap exists to contain.
21. **A written wait is refused where `wire()` can see it.** `.backoff(..)` on an app with no `Timer` is a wiring error like any written bound. A standalone deadline on such an app is stored and never fires: it is a runtime value passed to `execute`, whose only error is `Closed`, and refusing it would widen that error for one rare case on an app `listen()` already confines to standalone use.
22. **Scope is its own axis.** `with = closure` says how a binding is built and nothing about how long it lives; its scope is `Auto`, with the meaning item 7 gives, and the same closure means the same thing on a method, on a controller impl, in an `into` list, in a `providers` list, where a bare closure is shorthand for it, and through the value API. One override serves every position: `with(singleton | execution | transient) = closure`, the three words `#[injectable(..)]` takes; `ModuleDef::singleton`/`execution`/`transient` and `Contribute::singleton`/`execution`/`transient` beside each one's `with`/`try_with`; `EnhancerSpec::guard_with_in::<S>` and kin beside `guard_with`. An explicit `singleton` that needs an execution is refused at `wire()` as a type's is. Inference reads what a closure reads, not what it does: a closure that creates per-call state and reads nothing writes `with(execution)`. `value = expr` is built once and shared wherever it is declared.
