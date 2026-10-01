# Divergences: the spine

Every place the `ulo` and `ulo-macros` skeleton departs from `DESIGN.md`, or fills in a public
shape the design leaves unspecified. Each entry gives what the design says, what the spine
writes, and why. All await the user's sign-off.

## Dependencies

### 1. `async-lock` as the once-cell

- **Design:** §3.8 asks for "a runtime-agnostic async once-cell (for example from `async-lock`,
  which is not a runtime)".
- **Spine:** `async-lock = "3.4"` in `crates/ulo/Cargo.toml`, crate-local rather than in
  `[workspace.dependencies]`, where no entry exists. It backs `ExecCache` and the shutdown outcome.
  The core has no other dependency besides `ulo-macros`.
- **Why:** the design's own example; the lockfile already resolves 3.4.2.

## Binding handles and the value API

### 2. `Handle<'m, T, K, C = K>`: a lifetime, and the binding's state in `C`

- **Design:** §9.1 describes one handle type "with a state parameter naming the item written last
  and whether its bound is written", carrying the construction's state across items. It names no
  lifetime and does not say how the scope reaches the type.
- **Spine:** `Handle<'m, T, K, C = K>`. `'m` borrows the binding record in the `ModuleDef`. `K` is
  the item (`Binding<S, B>`, `Contribution<S, B>`, `ReadyItem<W, A>`, `HookItem<B>`); `C` is the
  binding item's state, which carries the scope `S` and the bound `B`. Hooks and `.ready` exist
  where `C: HookHost`; `also_as` and `qualified` where `C: SingleBinding`. The markers and traits
  are public in `ulo::handle`.
- **Why:** the handle writes into the record it was returned for, which needs the borrow. Putting
  the scope in `C` gates the hook methods on the binding, not on the item written last, so they
  stay available after `.ready(..)` or another hook.

### 3. Closure hooks exist on `Auto` handles

- **Design:** §9.1 and §13: closure hooks on "singleton handles only".
- **Spine:** `HookHost` is implemented for `Binding<S: HookCapable, _>`, which admits `Auto`, so
  `m.provide::<T>()` for an `Auto` type carries `.on_destroy(..)` and `.ready(..)`.
- **Why:** an `Auto` provider is a singleton (§3.3), and `HookCapable` already admits `Auto` for
  trait hooks. An `Auto` binding inferred per-execution with hooks is refused at `wire()`, as for
  trait hooks (§6.2).

### 4. Which handles start with the construction bound written

- **Design:** silent on whether `.timeout(..)` exists after `provide`, `provide_with` or `value`.
- **Spine:** `provide::<T>()` returns `Binding<T::Scope, Set>`: `T::CONSTRUCT_TIMEOUT` writes the
  bound, so the handle has no `.timeout`/`.unbounded()`. `provide_with` and `try_provide_with`
  return `Open`: the factory replaces `T::construct`, which `CONSTRUCT_TIMEOUT` bounds, so the
  const does not apply and the handle writes the bound. `value`, `try_value` and
  `Contribute::value` return `Set`: nothing is built.
- **Why:** one source per bound (§9.1, §12). A typestate cannot read a const's value, so a const
  and an open handle on one item would be two sources.

### 5. Readiness checks only on singleton handles

- **Design:** §9.3 runs a check "right after its binding is constructed", failing as
  `ConnectError::Readiness`; it does not say which handles carry `.ready`.
- **Spine:** `.ready(..)` exists where `C: HookHost` (singleton or `Auto`).
- **Why:** `ConnectError` exists only in `connect`, which builds only singletons.

### 6. A second `.ready(..)` is a wiring error

- **Design:** silent.
- **Spine:** `WiringError::DuplicateReadiness` naming both locations.
- **Why:** replacing the first check would discard its bounds without a word, the kind of silent
  replacement [10] refuses for bindings.

### 7. Shutdown closures take the `Signal` first

- **Design:** §5 says hook closures accept any site; [39] says before-shutdown and shutdown hooks
  receive the signal; §9.1 shows no closure form of either.
- **Spine:** `before_shutdown` and `on_shutdown`, on handles and on `ModuleDef`, take
  `F: ShutdownFactory<Args>`: `|signal: Signal, pool: Dep<PgPool>| async move { .. }`.
- **Why:** gives closure hooks what trait hooks get, without making `Signal` a site that could be
  read where no shutdown is running.

### 8. `Factory<Args>` and `ShutdownFactory<Args>` are named public traits

- **Design:** never names the bound on factory, readiness, hook or enhancer closures.
- **Spine:** both traits public, implemented for closures of zero to twelve parameters, each
  parameter a `Site`. `Factory::Output` is the future's output.
- **Why:** they appear in every public signature that takes a closure.

### 9. `provide_with::<T>(factory)` cannot be called with one type argument

- **Design:** §4 and §9.1 write `m.provide_with::<Cache>(|| async { .. })`.
- **Spine:** `provide_with<T: Construct, Args>(&mut self, factory: impl Factory<Args, Output = T>)`.
  The call is written `provide_with::<Cache, _>(..)` or `provide_with(..)`, with `T` inferred from
  the factory. `try_provide_with` likewise, as `::<Cache, _, _>`.
- **Why:** `Args` must be a generic parameter (the arity impls differ only by it), a function's
  type parameters take no defaults, and supplying one of two is E0107. `also_as::<dyn U>` works
  because its closure is the only other generic and is written `impl Trait`; here `Args` sits
  inside the `impl Trait` bound and is not.

### 10. `Contribute<'m, U, Q = ()>`

- **Design:** shows `contribute::<T>().provide::<X>(|a| a)`, `.value(..)` and
  `.singleton(factory, |a| a)`; `Many<T, Q>` implies qualified collections but no spelling writes
  a contribution to one.
- **Spine:** `.qualified::<Q2>()` on the builder; `provide::<T>(coerce)`, `value(Arc<U>)`, and
  `singleton`, `try_singleton`, `execution`, `try_execution`, `transient`, `try_transient`, each
  taking `(factory, coerce)`. They return handles in a `Contribution` state, which has no
  `also_as` or `qualified`.
- **Why:** a contribution has no key of its own to add a second key to; every scope a binding can
  take, a contribution can take.

### 11. `export_qualified` and `reexport_qualified`

- **Design:** §13 lists `export` and `reexport`, both written with one type argument.
- **Spine:** `export::<T>()` and `reexport::<T>()` export `T @ ()`;
  `export_qualified::<T, Q>()` and `reexport_qualified::<T, Q>()` export `T @ Q`.
- **Why:** a binding qualified with `.qualified::<Q>()` outside a keyed module has no other way out.

### 12. `ModuleDef::controller::<C>()`

- **Design:** §3.6 says a module declares controllers; §13's table lists no method.
- **Spine:** `controller<C: Controller>(&mut self)`.
- **Why:** `#[module(controllers = [..])]` needs a value-API call to lower to.

### 13. Module hooks: four, returning `ModuleHook`

- **Design:** §13 lists `ModuleDef::{on_init, on_destroy}`; [39] says a module has hooks of its own.
- **Spine:** `on_init`, `on_destroy`, `before_shutdown`, `on_shutdown`, the same four a binding
  handle has, each returning `ModuleHook<'_, B>` whose bound is written once with `.timeout(..)`
  or `.unbounded()`.
- **Why:** matches the handle's set; a module holding a resource needs the signal-receiving hooks
  as much as a binding does.

### 14. `Module::keyed::<Q>()` is a provided method

- **Design:** §3.6 gives `Module` two methods; §8.3 writes `DbModule::for_root(url).keyed::<Primary>()`.
- **Spine:** `fn keyed<Q: 'static>(self) -> Keyed<Q, Self> where Self: Sized` on `Module`.
- **Why:** reachable on every module without an import; `where Self: Sized` keeps `Module`
  dyn-compatible.

### 15. `DynamicModule::new` takes an `FnOnce`

- **Design:** §8.4's example moves `cfg` into an inner `move` closure, which makes the outer
  closure `FnOnce`; the signature is not written.
- **Spine:** `new<O: 'static, C: Eq + Hash + Clone + Send + Sync + 'static>(config: C, register:
  impl FnOnce(&mut ModuleDef<'_>) + Send + 'static)`, plus `label(self, name)`. The closure runs
  once; identities are deduplicated before `register` is called.
- **Why:** the design's own example does not compile against `Fn`.

### 16. `Meta` for `m.meta::<T>()`

- **Design:** §7: "The core stores typed per-module metadata that `fw-http` reads"; no shape.
- **Spine:** `trait Meta: Default + Send + Sync + 'static { fn sites(&self, s: &mut Sites) {} }`;
  `ModuleDef::meta<T: Meta>(&mut self) -> &mut T`; `ModuleRef::meta<T: Meta>(&self) ->
  Option<Arc<T>>`. `load` refuses a module that wrote any metadata as `LoadRefusal::Middleware`.
- **Why:** the core cannot know which metadata is middleware, and a transport reads all metadata
  when it binds, which a lazy module comes after.

### 17. `Secret<T>`

- **Design:** names `Secret<String>`, `.expose()`, `impl Into<Secret<String>>`, and the three ways
  a secret is registered; no shape.
- **Spine:** `new`, `expose`, `into_inner`; `From<String>` and `From<&str>` for `Secret<String>`;
  `Debug` and `Display` print `[redacted]`; `Clone`, `PartialEq`, `Eq` and `Hash` by value.
  `ModuleDef::secret<T: Display>(&mut self, &Secret<T>)`. `m.value(..)` registers a
  `Secret<String>` it binds, recognized by downcast.
- **Why:** a configured module derives `Eq + Hash` over its secrets. A generic `value<T>` can only
  recognize a secret by downcast, so other `Secret<T>` values bound by value are not registered;
  `m.secret` covers them.

## Keys, sites and the resolver

### 18. `KeyName`, and `Key`'s `Display`

- **Design:** §3.1 gives `Key`'s `Display` as `"PgPool"`, `"PgPool @ Replica"`,
  `"dyn Plugin (collection)"`; `KeyName` appears in every error with no shape.
- **Spine:** `Key`'s `Display` writes the first two forms. `KeyName` holds the `Key` and a
  `BindingKind`, writes all three, and exposes `key()` and `kind()`.
- **Why:** a `Key` carries no kind, so it cannot know it names a collection. `key()` lets a caller
  compare an error's key with `Key::of::<T, Q>()`.

### 19. `ModuleName`, `BindingKind`, `HookKind`, `ScopeKind`

- **Design:** names all four without listing them.
- **Spine:** `ModuleName` (`Display`, `as_str`); `BindingKind { Single, Collection }`; `HookKind`
  with one variant per hook trait; `ScopeKind { Singleton, PerExecution, Transient, Auto }`. None
  is `#[non_exhaustive]`.
- **Why:** each is a closed set the design fixes; §10.2's attribute rule covers error types.

### 20. `LookupKind::Module`

- **Design:** `NotFound { key, kind: LookupKind }`, commented "binding, extension, input".
- **Spine:** `LookupKind { Binding, Extension, Input, Module }`.
- **Why:** `app.module::<M>()` with no module of type `M` needs an answer, and `LookupError` has no
  other variant for it.

### 21. `Resolver::dep` and `many` read the unqualified key

- **Design:** §13 lists `Resolver::{dep, many, ..}` and writes `r.dep::<PgPool>().await?`.
- **Spine:** `dep::<T>() -> Dep<T>` and `many::<T>() -> Many<T>`; `dep_qualified::<T, Q>()` and
  `many_qualified::<T, Q>()` for a qualifier.
- **Why:** the design's spelling needs one type parameter, and a function's parameters take no
  defaults.

### 22. The rest of the `Resolver` surface

- **Design:** names `entries`, `ext`, `input`, `module`, `execution`, `by_key`; no signatures.
- **Spine:** `entries::<T>() -> Result<Entries<'_, T>, LookupError>`, an exact-size iterator of
  `Entry` with `resolve() -> Result<Arc<T>, LookupError>`; `ext::<T>()` and `input::<T>()`
  synchronous; `module() -> ModuleRef`; `execution() -> Result<ExecutionRef, LookupError>`;
  `by_key::<T>(key) -> Result<Arc<T>, LookupError>`.
- **Why:** `by_key` answers `Arc<T>` because an erased key's qualifier is not a type a `Dep<T, Q>`
  could carry. Role collections are unqualified, so `entries` takes no qualifier.

### 23. `Sites` and `SiteDesc` methods

- **Design:** names both types; §13's example calls `s.site::<Dep<PgPool>>()`.
- **Spine:** `Sites::{field::<S>(name), param::<S>(name), site::<S>()}`, the names feeding
  diagnostics. `SiteDesc::{dep(Key), many(Key), ext::<T>(), execution(), module(), optional::<S>()}`.
- **Why:** an integration's own site type describes what it reads through these.

### 24. Additive trait impls

- **Design:** lists `Deref`, `Clone` and `iter()` for the site types.
- **Spine:** also `IntoIterator for &Many`, `IntoIterator for &WiringErrors`, and `Clone` on `Ext`.
- **Why:** conveniences with no behaviour of their own.

## Executions and the app

### 25. `Execution::open` and `open_terminal` take the module and options

- **Design:** names `Execution::{open, open_terminal(&DrainToken, ..)}` without the rest.
- **Spine:** `open(module: &ModuleRef, opts: ExecOptions)`,
  `open_terminal(token: &DrainToken, module: &ModuleRef, opts: ExecOptions)`, both
  `-> Result<Execution, Closed>`.
- **Why:** an execution resolves with a module's visibility (§3.8), and a transport opens it in
  the dispatching controller's module, which `MountedHandler::module` hands it.

### 26. `cancel()` and `resolver()` on executions

- **Design:** the transport fires cancellation (§3.8) and reads `Resolver::entries` (§7); no
  method does either.
- **Spine:** `cancel(&self)` and `resolver(&self) -> Resolver<'_>` on `Execution` and
  `ExecutionRef` alike.
- **Why:** a disconnect is often detected where only a clone is held, such as a dropped streaming
  body; §3.8 gives `ExecutionRef` the same methods as `Execution` except `seed`.

### 27. `ExecOptions` and `Extensions`

- **Design:** `ExecOptions::new().deadline(..)`; `Extensions` is "a typed bag" written by
  `insert`.
- **Spine:** `ExecOptions::{new, deadline}`. `Extensions::{insert, get -> Option<Arc<T>>,
  contains, remove}`, all through `&self`.

### 28. The `Bound` typestate lives at `ulo::app::Bound`

- **Design:** uses `Bound` for both the timeout enum (§3.9) and the typestate after `listen()` (§2).
- **Spine:** `ulo::Bound` is the timeout enum; the typestate is `ulo::app::Bound`, beside
  `Wired` and `Connected`, which the root also re-exports. All three are uninhabited enums.
- **Why:** one namespace cannot hold both.

### 29. The builder and the typestate methods

- **Design:** §9.4 shows `App::builder(..).timer(..)..wire()?`, `connect()`, `.bind(..).bind(..)
  .listen()`, `serve(signal)`, and `close(..)` on a `Connected` app.
- **Spine:** `App::builder(root: impl Module) -> AppBuilder`; `AppBuilder::wire(self) ->
  Result<App<Wired>, StartupError>`; `App<Connected>::bind(self, server: impl Server) ->
  App<Connected>`, `listen(self) -> Result<App<app::Bound>, StartupError>`, `close(self, signal)`
  consuming the app; `App<app::Bound>::serve(self, signal: impl Future<Output = Signal> + Send)`.
  `load`, `draining` and `is_draining` are on `AppHandle` alone.
- **Why:** the shapes the design's calls need; §8.6 places `load` and the drain notice on the handle.

### 30. `module::<M>()` names the identity's type

- **Design:** `app.module::<UsersModule>()`, `app.module_keyed::<DbModule, Replica>()`.
- **Spine:** `module<M: 'static>` and `module_keyed<M: 'static, Q: 'static>`, on `App<Connected>`
  and `AppHandle`, matching the type a module's identity names; for a `DynamicModule` that is the
  owner type, which is not itself a `Module`.
- **Why:** a `M: Module` bound would make a dynamic module unreachable.

### 31. `ModuleRef::{name, meta}`

- **Design:** `ModuleRef` carries `get` and `execute`.
- **Spine:** also `name() -> ModuleName` and `meta::<T>()` (entry 16). `ModuleRef` is `Clone`.

### 32. `TestApp<S = Settled>`

- **Design:** §11's chain: `override_*(..)`, optionally `.in_module*`/`.everywhere()`, then the
  next call.
- **Spine:** after an `override_*` the builder is `TestApp<Pending>`, which alone has
  `in_module::<M: 'static>()`, `in_module_keyed::<M, Q>()`, `in_module_of(&config)` (any
  `M: Module`, through its identity) and `everywhere()`. `override_many(items: impl
  IntoIterator<Item = Arc<T>>)`. Besides `timer`, the four builder knobs are forwarded, and
  `wire()` is offered beside `connect()`. An override keeps the replaced binding's scope.
- **Why:** scoping an override that is not the last call would scope the wrong one; a shutdown
  test needs the knobs.

## Transports and enhancers

### 33. `Server`

- **Design:** "`Server` (implemented by transports for `bind` and the drain, which hands over the
  `DrainToken`)".
- **Spine:** `type Transport: Transport`; `bind(&mut self, Mounted<'_, Self::Transport>)`,
  `serve(&self)`, `drain(&self, DrainToken)`, `close(&self)`, each returning `impl Future + Send`,
  the fallible ones `Result<(), BoxError>`. `Mounted` exposes `handlers()`, `app()` and `timer()`.
- **Why:** `serve` drives the accept loop and `close` is step 6 of §9.5; a transport enforces
  per-call deadlines on the app's `Timer`, which `listen()` guarantees.

### 34. `Controller`, `Mount`, `MountedHandler`

- **Design:** names `Controller::mount` and `Mount`.
- **Spine:** `trait Controller: Construct { fn mount(m: &mut Mount<'_>); }`;
  `Mount::handler::<T, H>(name, controller: EnhancerSpec<T>, method: EnhancerSpec<T>, handler: H)`;
  `MountedHandler<T>::{name, controller, module, handler::<H>()}`. `H` is the transport's own
  handler value; the core stores it erased and hands it back by type.
- **Why:** the core cannot name a transport's routes, patterns or methods.

### 35. `EnhancerSpec` methods

- **Design:** §7 writes `spec.interceptor::<TimingInterceptor>()`.
- **Spine:** `guard::<G>()`, `guard_value(g)`, `guard_with(factory)`, and the same three for
  interceptors and error handlers. One spec is one tier; a handler gets two.
- **Why:** by type, by value and by closure are the three forms [26] names.

### 36. A by-type enhancer must be bound where the controller can see it

- **Design:** "by type (resolved from the container)"; silent on whether the type must be bound
  by a module.
- **Spine:** a by-type declaration is a dependency on the enhancer's own key, resolved against the
  controller module's visibility; a type no visible module binds is a missing dependency at
  `wire()`, naming the handler.
- **Why:** "resolved from the container" read literally; no binding is created implicitly.

### 37. `Next::run(self)`

- **Design:** `Next<'_, T>` is passed to `intercept`; no method is named.
- **Spine:** `run(self) -> BoxFuture<'a, Result<T::Reply, BoxError>>`, with no `Cx` argument.
- **Why:** every clone of `Cx` is the same call, so the chain continues with the one it holds.

### 38. The erased twins, and `ErasedGuard::name`

- **Design:** "dyn-compatible twins with blanket impls"; no signatures.
- **Spine:** each method returns a `BoxFuture` borrowing `self` and the `Cx`; `ErasedGuard` adds
  `name() -> &'static str`, the guard's type name.
- **Why:** `GuardRejected { guard }` needs the name, and a role collection holds only the twin.

### 39. What an error handler's `Err` means

- **Design:** errors go through the handlers method first, then controller, then global; silent on
  what a handler's `Err` does.
- **Spine:** `Ok` claims the error with a reply; `Err` hands the next handler the error returned,
  the same one or a reshaped one. The last `Err` reaches the transport.

### 40. `ulo::dispatch`

- **Design:** §7 describes the pipeline as the transport's.
- **Spine:** a public `dispatch(handler, exec, cx, call)` runs the guards, interceptors, handler
  and error handlers in the §7 order; transports call it.
- **Why:** four transports would otherwise each implement the lazy guard walk and the error order.

### 41. The `#[routes]` handler protocol

- **Design:** §7 requires a controller-level enhancer to be checked per handler against that
  handler's transport, with an error naming the handler; the handler attributes belong to the
  transport crates.
- **Spine:** `#[routes]` appends `#[::ulo::__private::__handler(..)]` to each handler; the
  transport's attribute consumes it, calls `::ulo::__private::__enhancer_specs!(<Transport>,
  "<key>", ..)` to build both tiers with one role assertion per enhancer, and writes
  `__ulo_mount_<name>`, which `impl Controller` calls. Transport-scoped entries
  (`http = AuthGuard`) are filtered by the key the transport passes.
- **Why:** `#[routes]` expands before the method attributes and cannot know a handler's transport
  type; the transport attribute can. Both items are `#[doc(hidden)]`, but transport crates write
  against them.

### 42. Enhancer attribute names and grammar

- **Design:** §7 shows `#[guards(..)]` and `#[interceptors(..)]` with `Type`, `http = Type`,
  `value = expr` and `with = closure`.
- **Spine:** adds `#[error_handlers(..)]` with the same forms, and `key(value = ..)` /
  `key(with = ..)` for transport-scoped values and closures. A `with` closure's body is wrapped as
  `async move { .. }` before it reaches `*_with`, so the closure is written synchronously as in §7.

## Errors

### 43. `WiringErrors` and `WiringError`

- **Design:** one entry per §10.1 failure; no variant list.
- **Spine:** `WiringErrors::{iter, len, is_empty}` and `IntoIterator`; `WiringError` is
  `#[non_exhaustive]` with one variant per failure §10.1 names (import cycle, re-export not
  visible or ambiguous, keyed input, duplicate binding, kind mix, dangling alias, failed value,
  duplicate readiness, the five override and replacement failures, missing, ambiguous, cycle,
  scope violation, hooks on a per-execution binding, unseeded input, bound without a `Timer`,
  knob without a `Timer`). Fields are `KeyName`, `ModuleName`, source locations, and `String`
  where a report prints a consumer or a path.

### 44. `Display`, `Error` and `From` on the error types

- **Design:** shapes only.
- **Spine:** every error type implements `Display` and `Error`, `ConstructError` included;
  `StartupError` has `From<WiringErrors>` and `From<ConnectError>`. `NoTimer`, `Closed` and
  `GuardRejected` carry their text in the spine.
