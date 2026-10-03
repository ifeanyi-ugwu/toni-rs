# Divergences from `DESIGN.md`: sign-off

Every place the built `ulo` and `ulo-macros` depart from `DESIGN.md`, or fill a gap it leaves,
gathered from the nine logs under `divergences/`. Nothing here has compiled yet.

Each item cites its log in brackets: `[spine 9]` is entry 9 of `spine.md`, `[C 3]` entry 3 of
`C.md`, `[A R4]` request 4 in `A.md`, `[C E1]` request E1 in `C.md`, `[B 3 = E 4]` one item two logs
record. The log carries the full reasoning. The design's section numbers are the logs' own.

Section 1 needs an answer per item, by number. Sections 2 to 6 need a read; an objection to any
entry is a decision too.

---

## 1. Decisions for you

**D1. A shutdown closure hook whose site read fails reports `Errored`.** [B 3 = E 4]
§10.2 says `ShutdownFailure::Hook` is "never `Errored`: the shutdown hook traits return `()`". A
closure hook (`.on_destroy(|pool: Dep<PgPool>| ..)`) reads sites, and a read that fails means the
hook never ran. Built: the erased hook answers `Err`, stored as `FailureReason::Errored(Redacted)`,
the `LookupError` reachable by downcast; the same for an `on_init` closure as
`ConnectError::Hook`.
Options: (a) keep `Errored` and amend §10.2; (b) a new `FailureReason` variant naming an unread
site; (c) drop the failure.
Recommendation: (a). The hook did not run and the report should say so; a variant can follow when a
second producer of it appears.

**D2. A dedicated `LookupError::Ambiguous`.** [A 4, A R4, E 11]
A runtime lookup of a key two visible modules bind (`ModuleRef::get`, `exec.get` inside an execution
opened in a non-root module, `by_key`) answers `AmbiguousModule { module, candidates }` with
`module` holding the key's type name. E's `Display`, "`X` is ambiguous between A, B", reads for
both.
Options: add `Ambiguous { key: KeyName, sources: Vec<ModuleName> }`; keep the reuse.
Recommendation: add it. `AmbiguousModule.module` carrying a binding misdescribes, and `KeyName`
lets a caller compare with `Key::of`.

**D3. A `WiringError::ExportNotBound` variant.** [C 3, C E1]
`export::<T>()` of a key the module does not bind reports `Missing` with the consumer "its export
list", or "its export list (a key an import provides leaves through `reexport`)" when an import
provides the key.
Options: add `ExportNotBound { module, key, imported, at }` with the help "bind `{key}` in {module}"
or "re-export it with `reexport`"; keep `Missing`.
Recommendation: add it. A hint carried inside a consumer string is text where a field belongs.

**D4. A `WiringError::ReplacementUnmatched` variant.** [C 14, C E2]
A `replace_module` whose original no module imports is applied to nothing and reported nowhere.
Options: add `ReplacementUnmatched { original, at }` in step 2; leave it.
Recommendation: add it. §11 reports an unmatched override as a stale mock, and a replacement is
the same fault.

**D5. Overrides for a binding written `.qualified::<Q>()`.** [D 6]
Every `override_*` targets `T @ ()`. Inside a keyed module that is the binding's own key, reached by
`in_module_keyed::<M, Q>()`. A binding qualified with `.qualified::<Q>()` outside a keyed module
(spine 11 made it exportable) has no override spelling.
Options: add `override_value_qualified::<T, Q>` and the factory kin; leave it.
Recommendation: add the three. A binding a test cannot replace is a hole in §11.

**D6. A controller-level entry scoped to a misspelled transport key is dropped without an error.**
[G 5]
`#[guards(htpp = AuthGuard)]` on a `#[routes]` impl matches no handler and applies to nothing.
`#[routes]` expands before the transport attributes and does not know its handlers' keys.
Options: accept until a transport crate exists, then add a `const __ULO_KEY_<name>` to the
protocol and compare in a const assertion; restrict keys to a fixed list, which refuses a
user-written transport (§6.3).
Recommendation: accept now and file it; add the key constant when the protocol is first written
against, together with D7.

**D7. A controller-level `value = expr` is built once per handler.** [G 3]
§7 and [26]: "by value: built once, shared". Built: each handler's `__ulo_mount_<name>` evaluates
the impl-level expression: a controller with three handlers builds three values, and a rate
limiter declared there limits each handler separately. Shared state is declared by type and bound
as a singleton.
Options: accept and document; change the protocol so `impl Controller` builds impl-level values
once and passes them down.
Recommendation: accept now; revisit with D6, since both need the same channel from
`Controller::mount` to the per-handler mount fns.

**D8. `.backoff(..)` on an app with no `Timer`.** [B R3, E 5]
§10.1 step 6 lists the bounds that need a `Timer`; `.backoff` is not among them, and nothing can
wait without one. Built: the next attempt follows at once, with no wiring error.
Options: keep; refuse at `wire()` as a knob needing a `Timer` (C's step 6).
Recommendation: refuse at `wire()`. A written wait that does not wait is the kind of discard the
design refuses elsewhere, and removing `.backoff` in a timerless job is one line.

**D9. Closure hooks and `.ready(..)` exist on `Auto` handles.** [spine 3, spine 5, C 8]
§9.1 and §13: closure hooks on "singleton handles only". Built: `m.provide::<T>()` for an `Auto`
type carries `.on_destroy(..)`, `.ready(..)` and the rest; an `Auto` binding inferred
per-execution with any of them is refused at `wire()` as `HooksOnPerExecution`, a `.ready` counted
as a hook.
Options: keep; restrict to explicit singletons.
Recommendation: keep. An `Auto` provider is a singleton (§3.3), and trait hooks already take the
same path.

**D10. Closure sites outside a binding are checked for keys, not scope.** [C 7]
§10.1 step 3 resolves "every site". Built: hook closures, readiness checks, module hooks, enhancer
closures and metadata sites are checked for missing and ambiguous keys only. A hook reading
`Ext<T>` passes `wire()` and fails at `connect` with `ExecutionRequired`.
Options: accept; add a `wire()` check for the sites that can never be satisfied there (an `Ext`,
`ExecutionRef`, input or per-execution key in a hook or readiness closure).
Recommendation: add the check. The fault is static, and `connect` is later and costlier than
`wire()`.

**D11. `WrongType` is reachable outside `by_key`.** [A 7]
§10.2: "`WrongType` is reachable from one surface only, `Resolver::by_key::<T>(key)`". Built:
`dep`, `many`, `Entry::resolve` and an input read answer it when the stored instance does not hold
the key's type after the recorded coercion; rule 9 forbids the panic. With consistent records it
cannot happen, and C 12's override rule (an `also_as` key under override moves to a binding of its
own) removes A's example.
Options: accept and amend §10.2 to "the one surface a caller reaches deliberately"; nothing else
to build.
Recommendation: accept and amend.

**D12. `#n` in module names, labelled or not.** [C 16]
§3.6: "`DbModule @ Replica`, or `DbModule #2` when unlabeled". Built: `#n` for the second and later
module of one type and qualifier in collection order, whether labelled or not.
Options: keep; `#n` only when unlabelled.
Recommendation: keep. §13's integration labels every `DbModule` configuration alike, and two modules
printed alike cannot be told apart.

**D13. A failed `connect` or `load` runs no hooks for what it built.** [D 4, D 5]
§9.2: the first failure stops the walk; silent on what was built. Built: `App::connect` returns
`StartupError::Connect` and drops the app; singletons built and init hooks run get no destroy or
shutdown hook, and resources release as the instances drop. A failed `load` additionally restores
the base graph and removes the singletons it built, and a retry wires again.
Options: accept; run destroy and shutdown hooks for the built prefix in reverse.
Recommendation: accept for the first compile. Partial teardown wants its own design row, since
§9.5's steps presuppose `Connected`.

**D14. A panic in a guard, interceptor, handler or error handler unwinds out of `dispatch`.** [F 4]
§14.5 catches panics in `connect`, `load`, `close` and a build inside a call; silent on the
pipeline. Built: the transport decides what a panicking call answers. The framework today delivers
`PanicRecovered` to the chain on every transport.
Options: keep, each transport wrapping `dispatch`'s future; catch in `dispatch` and offer a public
panic error to the handlers.
Recommendation: catch in `dispatch`. The reason spine 40 gives for `dispatch` existing, four
transports would each write it, applies here, and the error type is one small addition.

**D15. A lookup failure during `connect` with no deeper key.** [E 3]
§10.2: a `ConstructError::Site` is "reported as the `LookupError` it carries". `ConnectError` has no
variant carrying one. Built: a nested build's `Construct { key, reason }` becomes
`ConnectError::Construct` naming the deeper key; any other lookup error (the `NotReady` a
constructor's `ModuleRef::get` meets) becomes `ConnectError::Construct { key: <binding>, reason:
Errored(..) }`, the `LookupError` by downcast.
Options: keep; a `ConnectError::Lookup { key, module, error }` variant.
Recommendation: keep. The design's "name the deeper key" holds where there is one, and the rest
have no deeper failure to name.

**D16. A deadline on a timerless app is stored and never fires.** [D 1]
`execute` with `ExecOptions::deadline` and no `Timer`: `deadline()` reports it, cancellation never
fires. `execute` answers `Result<R, Closed>` and has no error to refuse it with.
Options: accept and document on `ExecOptions::deadline`; refuse, which widens `execute`'s error.
Recommendation: accept and document. §12 already confines a timerless app to standalone use.

**D17. `Sites::site` is a chosen name.** [design-notes 1]
No document spelled a method on `Sites`; §13's `Construct` example calls `s.site::<Dep<PgPool>>()`
and the spine built `Sites::{field, param, site}` (spine 23).
Recommendation: accept.

**D18. `Resolver::dep` and the qualifier.** [design-notes 2 = spine 21]
§13 writes `r.dep::<PgPool>().await?`. A `Q` parameter on `dep` breaks that spelling, since a
function's type parameters take no defaults. Built: `dep::<T>()` and `dep_qualified::<T, Q>()`, the
same pair for `many`. Alternative: `Dep::<PgPool>::read(r)`, which the `Site` trait already
supports, as the only qualified spelling.
Recommendation: keep the pair; §13's example stands as written.

**D19. §13's pool binding binds a `Result`.** [design-notes 3]
`m.singleton(|| sqlx::connect_lazy(..))` binds `Result<PgPool, Error>` under the key, and the
`.ready(|pool: Dep<PgPool>| ..)` beside it reads a key nothing binds.
Options: `try_singleton`; sqlx's `connect_lazy_with`, which returns the pool.
Recommendation: `try_singleton`. It shows the `try_` form the row exists for.

**D20. Derive `Clone` on the internal records.** [C report; `graph/wire.rs` `clone_graph`]
A lazy load deep-copies the frozen graph through hand-written `clone_record`, `clone_ready`,
`clone_hook`, `clone_recipe`, `clone_handler`, `clone_module`, `clone_binding`, `clone_table` and
`clone_edge`, each naming every field of records B, F and C own. Any field added, renamed or made
private before the first compile breaks `wire.rs` from outside the area that changed it.
Options: derive `Clone` on the records and replace the functions with `base.clone()`; keep the hand
copies.
Recommendation: derive. The functions copy each field with `Copy`, `Arc::clone` or `.clone()`
already; a derive is the same copy, maintained by the compiler.

---

## 2. What you will write differently

Bindings and the value API:

- **`provide_with` takes two type arguments or none.** [spine 9] `Args` has to be a generic
  parameter and a function's type parameters take no defaults, so supplying one of two is E0107.
  Design: `m.provide_with::<Cache>(|| async { .. })`
  Built: `m.provide_with::<Cache, _>(|| async { .. })` or `m.provide_with(|| async { Cache::new() })`;
  `try_provide_with::<Cache, _, _>(..)` likewise.
- **Which handles carry `.timeout(..)` / `.unbounded()`.** [spine 4] `provide::<T>()` writes the
  bound from `T::CONSTRUCT_TIMEOUT`, and `value`/`try_value`/`Contribute::value` build nothing, so
  none of them has `.timeout`. `provide_with` and `try_provide_with` do.
- **Shutdown closures take the `Signal` first.** [spine 7] `before_shutdown` and `on_shutdown`, on
  handles and on `ModuleDef`:
  `.on_shutdown(|signal: Signal, pool: Dep<PgPool>| async move { .. })`.
  `Signal` is not a site, so it cannot be read where no shutdown is running.
- **`Factory<Args>` and `ShutdownFactory<Args>` are public traits** [spine 8], implemented for
  closures of zero to twelve parameters, each a `Site`; they appear in every signature taking a
  closure.
- **The handle type is `Handle<'m, T, K, C = K>`.** [spine 2] `'m` borrows the binding record; `K`
  is the item written last; `C` carries the scope and whether the bound is written. Hooks and
  `.ready` exist where `C: HookHost`; `also_as` and `qualified` where `C: SingleBinding`. The
  markers live in `ulo::handle`.
- **`.ready(..)` exists on singleton and `Auto` handles only** [spine 5], since `connect` builds
  only singletons (D9 covers the `Auto` part).
- **Contributions: `Contribute<'m, U, Q = ()>`.** [spine 10] `.qualified::<Q2>()` on the builder;
  `provide::<T>(coerce)`, `value(Arc<U>)`, and `singleton`/`try_singleton`/`execution`/
  `try_execution`/`transient`/`try_transient`, each `(factory, coerce)`. The handle returned has no
  `also_as` or `qualified`.
  `m.contribute::<dyn Plugin>().qualified::<Admin>().provide::<AuditPlugin>(|p| p)`
- **Qualified exports.** [spine 11] `export::<T>()` and `reexport::<T>()` export `T @ ()`;
  `export_qualified::<T, Q>()` and `reexport_qualified::<T, Q>()` export `T @ Q`, the only way out
  for a binding qualified outside a keyed module.
- **`Secret<T>`.** [spine 17] `new`, `expose`, `into_inner`; `From<String>` and `From<&str>` for
  `Secret<String>`; `Debug`/`Display` print `[redacted]`; `Clone`, `Eq`, `Hash` by value. A
  `Secret<String>` bound with `m.value(..)` is registered for redaction by downcast; any other
  `Secret<T>` needs `m.secret(&secret)` (`T: Display`).

Modules:

- **`ModuleDef::controller::<C>()`** [spine 12] is what `#[module(controllers = [..])]` lowers to.
- **Module hooks are four, each returning `ModuleHook<'_, B>`.** [spine 13] `on_init`, `on_destroy`,
  `before_shutdown`, `on_shutdown`, the same set a binding handle has; the bound is written once
  with `.timeout(..)` or `.unbounded()`. The design listed two.
- **`Module::keyed::<Q>()` is a provided trait method** [spine 14] (`where Self: Sized`, keeping
  `Module` dyn-compatible), so `DbModule::for_root(url).keyed::<Primary>()` works without an import.
- **`DynamicModule::new` takes an `FnOnce`, and gains `label`.** [spine 15]
  `DynamicModule::new::<RedisIntegration, _>(cfg, move |m| { .. }).label("redis")`. The closure runs
  once, after identities are deduplicated. The design's own example does not compile against `Fn`.
- **Typed per-module metadata: `Meta`.** [spine 16] `trait Meta: Default + Send + Sync + 'static {
  fn sites(&self, s: &mut Sites) {} }`; `m.meta::<T>() -> &mut T` on `ModuleDef`;
  `ModuleRef::meta::<T>() -> Option<Arc<T>>`.
- **In a `#[module]` providers list, a bare path is a type.** [G 7] `UserService` and `db::Pool<Pg>`
  lower to `provide`; `Config::default()` is an expression. A constant reads as a type and fails at
  `provide::<..>()`.
  Design: `providers = [LIMITS]`
  Built: `providers = [{ LIMITS }]`

Keys, sites and the resolver:

- **Qualified reads are separate methods.** [spine 21 = design-notes 2; D18]
  `r.dep::<PgPool>()` stays; a qualifier is `r.dep_qualified::<PgPool, Replica>()`; `many` and
  `many_qualified` likewise.
- **The rest of `Resolver`.** [spine 22] `entries::<T>() -> Result<Entries<'_, T>, LookupError>`, an
  exact-size iterator of `Entry` with `resolve() -> Result<Arc<T>, LookupError>`; `ext::<T>()` and
  `input::<T>()` synchronous; `module() -> ModuleRef`; `execution() -> Result<ExecutionRef, _>`;
  `by_key::<T>(key) -> Result<Arc<T>, _>` (an erased key's qualifier is no type for a `Dep<T, Q>`).
  `entries` takes no qualifier: role collections are unqualified.
- **`Sites` and `SiteDesc`.** [spine 23; D17] `Sites::{field::<S>(name), param::<S>(name),
  site::<S>()}`; `SiteDesc::{dep(Key), many(Key), ext::<T>(), execution(), module(),
  optional::<S>()}`.
- **The small named types.** [spine 18, spine 19, spine 20] `KeyName` holds a `Key` plus a
  `BindingKind` and exposes `key()`/`kind()`; `ModuleName` has `Display` and `as_str`;
  `BindingKind { Single, Collection }`, `HookKind` (one per hook trait), `ScopeKind { Singleton,
  PerExecution, Transient, Auto }`, none `#[non_exhaustive]`; `LookupKind` gains `Module` for
  `app.module::<M>()` with no such module.

Executions and the app:

- **`Execution::open` and `open_terminal` take the module and options.** [spine 25]
  `Execution::open(&module, ExecOptions::new())`, `open_terminal(&token, &module, opts)`, both
  `-> Result<Execution, Closed>`; an execution resolves with that module's visibility.
- **`cancel()` and `resolver()` on `Execution` and `ExecutionRef` alike.** [spine 26] A disconnect is
  often detected where only a clone is held.
- **`ExecOptions::{new, deadline}`; `Extensions::{insert, get -> Option<Arc<T>>, contains, remove}`**,
  all through `&self`. [spine 27]
- **Two `Bound`s.** [spine 28] `ulo::Bound` is the timeout enum; the typestate after `listen()` is
  `ulo::app::Bound`, beside `app::{Wired, Connected}`, all three re-exported at the root.
- **The builder chain's shapes.** [spine 29]
  `App::builder(root).timer(t).wire()?` → `App<Wired>`; `.connect().await?` → `App<Connected>`;
  `.bind(http).bind(rpc).listen()?` → `App<app::Bound>`; `.serve(signal).await` with `signal: impl
  Future<Output = Signal> + Send`; `close(self, signal)` consumes a `Connected` app. `load`,
  `draining` and `is_draining` are on `AppHandle` alone.
- **`module::<M>()` bounds `M: 'static`, not `M: Module`** [spine 30], so a `DynamicModule` is found
  by its owner type, which is not itself a `Module`. The spelling `app.module::<UsersModule>()` is
  unchanged.
- **`ModuleRef` is `Clone` and adds `name()` and `meta::<T>()`.** [spine 31]
- **`TestApp<S = Settled>`.** [spine 32] An `override_*` returns `TestApp<Pending>`, which alone has
  `in_module::<M>()`, `in_module_keyed::<M, Q>()`, `in_module_of(&config)` and `everywhere()`, so a
  scope cannot attach to the wrong override. `override_many(items: impl IntoIterator<Item =
  Arc<T>>)`. Besides `timer`, the builder's four knobs are forwarded, and `wire()` is offered
  beside `connect()`.
  `.override_value::<Mailer>(fake).in_module::<UsersModule>().connect().await?`

Transports and enhancers:

- **`Server`.** [spine 33] `type Transport: Transport`; `bind(&mut self, Mounted<'_, Self::Transport>)`,
  `serve(&self)`, `drain(&self, DrainToken)`, `close(&self)`, each an `impl Future + Send`, the
  fallible ones `Result<(), BoxError>`. `Mounted` exposes `handlers()`, `app()` and `timer()`.
- **`Controller`, `Mount`, `MountedHandler`.** [spine 34] `trait Controller: Construct { fn mount(m:
  &mut Mount<'_>); }`; `Mount::handler::<T, H>(name, controller: EnhancerSpec<T>, method:
  EnhancerSpec<T>, handler: H)`; `MountedHandler<T>::{name, controller, module, handler::<H>()}`,
  `H` the transport's own value, stored erased and handed back by type.
- **`EnhancerSpec`'s nine methods.** [spine 35] `guard::<G>()`, `guard_value(g)`,
  `guard_with(factory)`, and the same three for interceptors and error handlers; one spec is one
  tier.
- **`Next::run(self)`** with no `Cx` argument [spine 37]: every clone of `Cx` is the same call.
- **`ulo::dispatch(handler, exec, cx, call)`** [spine 40] runs guards, interceptors, handler and
  error handlers in §7's order; transports call it rather than each writing the walk.
- **Enhancer attributes.** [spine 42] `#[error_handlers(..)]` joins `#[guards]` and
  `#[interceptors]`, all with `Type`, `http = Type`, `value = expr`, `with = closure`, plus
  `http(value = ..)` and `http(with = ..)` for transport-scoped values and closures.
- **A `with` closure may be sync or async.** [G 6] A sync body is wrapped as `async move { .. }`
  (spine 42); a closure already `async |..| ..`, or whose body is an `async` block, is passed as
  written; a `-> T` return type is kept on a binding inside the block.
- **A method-level entry scoped to another transport is a compile error.** [G 4]
  Written: `#[guards(http = AuthGuard)]` on an RPC handler
  Error on the key: "`get_rpc` is a `rpc` handler, so an entry scoped to `http` applies to nothing;
  write it unscoped, or on the handler it belongs to".
- **Any non-inert attribute makes a method a handler.** [G 8] In a `#[routes]` impl, a method
  carrying any attribute other than the language's own (`doc`, `allow`, `warn`, `deny`, `expect`,
  `cfg`, `cfg_attr`, `inline`, `must_use`, `deprecated`, `track_caller`) or an enhancer attribute
  is a handler. A helper with `#[tracing::instrument]` fails with "this handler's transport
  attribute did not read its enhancers"; put such helpers in a separate `impl` block.

---

## 3. Behaviour that differs or was filled in

Construction, readiness and hooks:

- **A factory reads its parameters in order, and the first failed read ends the call.** [B 1] No
  later site read starts, and nested constructions run in the order the closure lists them.
- **Readiness defaults: one attempt, no wait; `retries(n)` is `n` attempts after the first.** [B 2]
- **One binding's hooks run trait hooks first, then closure hooks in the order written.** [B 4, E 1]
- **Hook order across modules.** [E 1] At startup a module's own hooks run right after the last of
  its singletons in connect order; a module with no singleton runs after the modules before it in
  collection order. Each shutdown step runs lazily loaded groups first, latest load first, then the
  base graph, each in exact reverse of its startup order.
- **A second `.ready(..)` is `WiringError::DuplicateReadiness` naming both locations** [spine 6, B R1];
  **a second `.qualified::<Q>()` replaces the first** [B 5]. The asymmetry is deliberate: a replaced
  check would lose its bounds, a replaced qualifier conflicts with nothing.
- **An alias is a transient record** [B 6] with no sites and `Recipe::Alias { target }`; its target's
  scope decides sharing, and a path printed through it says `(transient)`.
- **Work that completes in the poll its bound fires in counts as done.** [B 8] `timeout` polls the
  future before the sleep.
- **A readiness check's reads are connect-order edges.** [C 7] What a check reads, other than its
  own binding, is built before the binding and counts in the cycle check.
- **A module hook's failure names the module's identity type as a key** (`DbModule @ Replica`); a
  contribution's names its collection (`dyn HealthIndicator (collection)`). [E 2] A readiness
  check's failed site read is an attempt's `Err` and is retried like one. [E 3]

Lookups:

- **A collection nothing contributes to reads as empty**, not `NotFound`; `Option<Many<T>>` is
  `Some` with no items. A single binding or input under the key is `WrongKind`. [A 3]
- **`by_key` locates the key before checking the type**, and builds nothing on either failure. [A 6]
- **An input read outside an execution is `ExecutionRequired`**, not `NotFound`, so
  `Option<Dep<RequestHead>>` propagates it; `Ext<T>` names `T`, `ExecutionRef` names itself. [A 8]
- **A key two visible modules bind is `AmbiguousModule`** from `ModuleRef::get`, a non-root
  `exec.get` and `by_key`; `Option<S>` propagates it rather than answering `None`. [A 4; D2]
- **`module::<M>()` with no qualifier matches every instance of `M`**, keyed or not, so a type
  imported bare and keyed is `AmbiguousModule` without one; none is `NotFound { kind: Module }`. [C 15]

Visibility and the wiring pass:

- **No shadowing.** [C 1] A key a module binds and also sees through an import or a global is
  `Ambiguous`; one binding reached by two routes is one source.
- **The root's table is swept whole** after site resolution: every unreported `Ambiguous` entry is
  reported, read or not, with the consumer "a lookup that names no module". Other tables report
  only what a site reads. §8.2's claim needs this; §10.1 step 3 says less. [C 2]
- **Execution inputs sit in every module's table**, below a binding under the same key. An input
  declared twice, or bound as a single anywhere, is `DuplicateBinding` naming both. [C 4]
- **A single/collection mix is app-wide**: one `KindMix` per key, naming the single's module. [C 5]
- **A contribution under `AnyGuard<T>`, `AnyInterceptor<T>` or `AnyErrorHandler<T>` is an enhancer
  even when no handler names `T`.** [C 6] The families are recognised by the `type_name` prefix of
  the role trait objects, read from a private probe transport. A compiler changing `type_name`'s
  format would demote such contributions to providers, which `wire()` then refuses as `Auto`
  providers needing an execution.
- **A refused binding does not cascade.** [C 9] A singleton or `Auto` provider needing an execution
  is refused and stays a singleton for its readers; one violation, one error.
- **The input check's walk** [C 10] starts at the controller, the global contributions under the
  handler's role keys, its by-type enhancers and what its closures read, entering execution-scoped
  and transient bindings only; each input reported once per handler and reading binding.
- **One cycle per strongly connected component**, the shortest through its smallest binding. [C 11]
- **A by-type enhancer is a dependency on its own key**, resolved against the controller module's
  visibility; unbound, it is `Missing` naming the handler. No binding is created implicitly.
  [spine 36, F R2]
- **A controller's key must be visible from its own module.** [F R2] An ambiguous controller key in
  its own module is reported.

Tests (`TestApp`):

- **What an override replaces and keeps.** [C 12] It replaces the recipe and the sites, and keeps
  the scope, `also_as` keys, hooks and readiness check. An override of an `also_as` key moves that
  key into a binding of its own in the same module. Keys match as written inside the module, so
  `override_value::<PgPool>(..).in_module_keyed::<DbModule, Replica>()` reaches the unqualified
  binding inside the keyed module. A `try_value` failure under an override is not reported.
- **`override_many` removes every contribution to the key** and contributes the items as values from
  the root, in order; none to remove is `OverrideUnmatched`, a single under the key `OverrideKind`.
  [C 13]
- **`replace_module` runs the original's `register` once**, into a node then dropped, to learn its
  exports. A replacement whose original is never imported is not reported (D4). [C 14]

Lazy loading:

- **A lazy module's contribution is refused** when the key already has contributions or anything
  reads it as a collection: a site, a closure, a module hook, a mounted handler's role key. [C 18]
- **One refusal, checked in collection order**: controllers, metadata, inputs, a global export, then
  contributions; a global module with no exports is accepted. [C 19] Any metadata written refuses
  the module as `LoadRefusal::Middleware`, since a transport reads metadata at bind. [spine 16]
- **The extended graph's connect order is recomputed whole**; the base order is its prefix. Each
  load's modules share one `loaded` number. [C, E R2]

Executions and the app:

- **A standalone deadline is enforced on the app's `Timer`**: `execute` polls `Timer::sleep` first,
  then the closure; when the sleep resolves cancellation fires and the closure runs on to its end;
  a deadline already past fires before the closure first runs. `Execution::open` stores a deadline
  and never enforces it. [D 1; D16]
- **`serve` polls every transport's `Server::serve`** beside its signal and, once triggered, beside
  the shutdown sequence. A `serve` failing before any trigger starts the shutdown under
  ``Signal::new("transport `<name>` failed: <error>")``, redacted; one returning `Ok` early triggers
  nothing; a failure after a trigger is dropped. [D 2]
- **`listen` with several transports.** [D 3] No `Timer`: refused before anything binds, naming the
  first transport queued. Otherwise transports bind in order; one failing closes those already
  bound and returns `StartupError::Bind`.
- **A failed `load` restores the base graph and removes what it built**, so a retry wires again.
  Loads are serialized. [D 4; D13]
- **An error handler's `Err` hands the next handler the error returned**, the same or reshaped;
  the last `Err` reaches the transport. [spine 39]

The pipeline:

- **The controller is built by `dispatch` before the chain** for singleton and per-execution
  scopes, so the handler closure's `exec.get::<C>()` reads what is there; a controller failing to
  build reaches the error handlers without the interceptors. A transient controller is built
  inside the chain, after every `next.run()`. [F 1]
- **Error handlers run last-declared first within a tier**: method, then controller, then global
  contributions in reverse collection order. [F 2]
- **An enhancer that cannot be built fails the call through the error handlers.** [F 3] A guard or
  interceptor failing to build ends the walk there; an error handler failing to build hands on its
  own `LookupError` in place of the one offered; a failure reading the global error-handler
  collection ends the walk with that error.

Shutdown:

- **The drain window waits for every transport's `drain` future** and for the live set to empty, or
  its deadline, where an unfinished `drain` is dropped. `close` runs per transport in reverse bind
  order, one after another, unbounded. [E 12]
- **A shutdown runner dropped mid-sequence resumes** from the recorded position in the next `serve`
  or `close` caller, with the same cap and deadline; a hook or `close` that started is not run
  again. [E 13]
- **A refused terminal execution is counted** in `Shutdown::terminal_skipped`, refused from the
  drain's end. [E R4, D additions]

Macros:

- **A generated `register` writes `global`, the `Secret<_>` fields, imports, providers, controllers,
  exports**, each list in the order written; controllers after providers fixes the connect-order
  tie-break so services precede what dispatches to them. [G 10]

---

## 4. Diagnostics and error text

Names:

- **Every path in a type name is cut to its last segment**, inside generic arguments too:
  `alloc::sync::Arc<my_app::db::PgPool>` prints `Arc<PgPool>`; a closure keeps its enclosing item,
  `main::{{closure}}`. Cost: `a::Config` and `b::Config` print alike in every diagnostic, `Key`'s
  `Debug` included; `KeyName::key()` still compares exactly. [A 1]
- **`Key` prints `PgPool` and `PgPool @ Replica`; `KeyName` adds ` (collection)`.** Both honour
  width and alignment (`{:<20}`). [A 2, spine 18]
- **A `WrongKind` key carries the kind the binding has**, `found`; `expected` carries the read's.
  [A 5]
- **A transport's name is its marker's type name after the last `::`** (`Http`); a generic marker
  keeps its full name. [F 5]
- **Module names** are `DbModule @ Replica` and `DbModule #2` (D12); the core's `Timer` module is
  `AppBuilder::timer`. [C 16]

Reports:

- **`Debug` on `StartupError`, `LoadError`, `ShutdownError` and `WiringErrors` writes their
  `Display` text**, so `main`'s `?` prints §10.1's report rather than a struct dump. The other
  error types keep a derived `Debug`. [E 9]
- **`StartupError` and `LoadError` are transparent**: the wrapped error's text, `Bind` as
  ``transport `X` failed to bind: {source}``, `LoadError::Closed` as "cannot load a module: the
  application is shutting down". `source()` answers the wrapped error's own source (`None` today),
  not the wrapped error, so a chain-walking reporter prints nothing twice. [E 10] `ConstructError`
  does the same, its `Debug` writing `Site(..)` or `Failed(..)` around the inner `Debug`. [B 7]
- **Every `WiringError` follows the sample's layout**: headline, `├─`/`└─` tree, help last.
  `Ambiguous` lists `needed by <consumer>` before the sources, aligned; `ScopeViolation` names the
  module, and an `Auto` provider's headline says it is a singleton because it is a provider, with
  §6.2's hint; paths print with ` → `, a cycle closed back to its first step. [E 11, E R3]
- **`FailureReason` texts**: `panicked: {message}`; "timed out after 2s (the attempt bound)", or
  "its own bound", "the app default", "`shutdown_timeout`"; "skipped: `shutdown_timeout` had
  expired"; else the error's text. `AmbiguousModule` reads "`X` is ambiguous between A, B". [E 11]
- **`WiringError` has one `#[non_exhaustive]` variant per §10.1 failure** (import cycle, re-export
  not visible or ambiguous, keyed input, duplicate binding, kind mix, dangling alias, failed value,
  duplicate readiness, the five override and replacement failures, missing, ambiguous, cycle, scope
  violation, hooks on a per-execution binding, unseeded input, bound and knob without a `Timer`);
  fields are `KeyName`, `ModuleName`, locations, and `String` for a consumer or path. Every error
  type implements `Display` and `Error`; `StartupError` has `From<WiringErrors>` and
  `From<ConnectError>`. [spine 43, spine 44]
- **The consumer strings C passes the report** [C, strings]: ``UserService (param `mailer`)``,
  ``UserService (field `repo`)``, `PgPool factory (param #1)`, `readiness check of PgPool (param
  #1)`, `OnModuleDestroy hook of PgPool (param #1)`, `OnModuleInit hook of UsersModule (param #1)`,
  ``metadata `Middleware` of UsersModule (param #1)``, ``UsersController::get (enhancer
  `AuthGuard`)``, `UsersController::get (enhancer closure) (param #1)`, `UsersController::get (its
  controller)`, ``the alias `PgPool @ ReadOnly` ``, `a lookup that names no module`. Path steps are
  the built type's short name with qualifier, then `(execution)` or `(transient)` when not a
  singleton; the last step is the site, ``Dep<RequestHead> (field `head`)``.
  `BoundWithoutTimer::item` reads ``construction of `PgPool` ``, ``readiness `.timeout` of
  `PgPool` ``, ``readiness `.attempt_timeout` of `PgPool` ``, ``` `OnModuleDestroy` hook of
  `PgPool` ```, ``` `OnModuleInit` hook of module UsersModule ```, printed as ``{item} needs a
  `Timer`, and the app has none``.
- **An input an enhancer closure reads directly** is reported with the path `handler (Transport) →
  enhancer closure → <site>`. [C 10]

Redaction:

- **A `Redacted`'s text is the message, then `: ` and each `source()` message not already
  contained**, up to 32 links, all redacted together; `source()` answers `None`. [E 6]
- **A panic payload** that is a `&'static str` or `String` is the message; anything else reads "a
  panic whose payload is not a string". [E 7]
- **The redaction function** replaces registered texts in one pass, longest first, never an empty
  one; treats `scheme://` as a URL's start and ends its authority at `/`, `?`, `#`, whitespace, a
  quote, a backtick or an angle bracket, replacing everything before the authority's last `@`. A
  password holding a raw `/` ends the authority early and is not stripped. `register_if_secret`
  recognizes `Secret<String>` and `Arc<Secret<String>>`. [E 8]

Compile errors:

- **A providers entry that is not `Construct`** fails on the entry as an unsatisfied `T: Construct`
  bound, with the trait's `on_unimplemented` text (section 6); the macro cannot see trait impls.
  [G 1]
- **A missing role** reports on the entry: "`AuthGuard` is not a guard for `Rpc`", the note
  "implement `Guard<Rpc>` for `AuthGuard`", and rustc's "required by a bound in `get_rpc`", which
  names the handler; the design wanted the handler in the message, which `on_unimplemented`
  cannot do. [G 2]
- **A field or parameter that is not a `Site` reports twice**, at the `sites` declaration and the
  `construct` read, both with the `Site` message; an `Ext` or `ExecutionRef` in an explicit
  singleton reports once. [G 9]

---

## 5. Internal only, listed for completeness

- `async-lock = "3.4"`, crate-local, backs `ExecCache` and the shutdown outcome; the core's only
  dependency besides `ulo-macros`. [spine 1]
- Additive impls: `IntoIterator for &Many`, `IntoIterator for &WiringErrors`, `Clone` on `Ext`.
  [spine 24]
- The erased enhancer twins return a `BoxFuture` borrowing `self` and the `Cx`; `ErasedGuard::name()`
  gives `GuardRejected { guard }` its name. [spine 38]
- The `__handler` protocol, for transport authors [spine 41, G 11]: `#[routes]` appends
  `#[::ulo::__private::__handler(name, controller(..), method(..))]` as the method's last
  attribute; the transport attribute removes it and passes its tokens to
  `::ulo::__private::__enhancer_specs!(<Transport>, "<key>", ..)`, a block evaluating to
  `(EnhancerSpec<T>, EnhancerSpec<T>)`; the transport writes `fn __ulo_mount_<name>(m: &mut
  ::ulo::Mount<'_>)` (`r#` stripped), which `impl Controller` calls in method order. A scope key is
  any identifier; `value` and `with` cannot be keys.
- `EnhancerSpec`'s methods return `&mut Self`, so the macro binds each tier to a local before
  passing it by value; `guard_with` infers `Args` from the closure's annotated parameters. [F R4]
- `MountedHandler<T>: Clone` by hand, without `T: Clone`; a server only borrows its handlers during
  `bind` and clones one per call. [F 6]
- `Resolver::instance(id)` stays `pub(crate)`, called by the pipeline for by-type enhancers and
  the controller; `Resolver<'a>` covariant and `Sync`, `Entries`/`Entry` `Send`. [F R1]
- Records: `provide`, `provide_with`, `try_provide_with` and `controller` set `hooks =
  erase_trait_hooks::<T>(location)` and `constructs = true`; `provide` and `controller` also set
  `construct_bound = T::CONSTRUCT_TIMEOUT`. Every contribution carries `into_primary: Some(..)`,
  `Contribute::value` the identity; a single binding `None`. [B R2, B R4, C]
- Inputs are stored in the `Instance` shape (`instance_of(Arc::new(v))`, read by
  `downcast_instance`) so `Dep<T: ?Sized>` can read one. [A R1, D]
- `obtain` answers in the binding's primary key type, applying `into_primary`; the resolver widens
  by the key looked up (direct downcast, then `into_primary` or the `also_as` entry matched by
  `TypeId`); an alias resolves its target through `graph.lookup(origin, target)` behind a named
  `BoxFuture`. A `coercion` handed a foreign instance returns it unchanged. [A R2, A R3, B, D]
- The singleton store holds the instance as built, unwidened, and hooks receive that instance.
  [D R2, E R4]
- Phases: `lifecycle::connect` leaves the phase alone; `App::connect` moves `Connecting` →
  `Running` around it; `load` runs it in `Running`. `close` runs the sequence when its trigger wins
  and joins the running one otherwise. [D R3, D R4, E R4]
- Frozen metadata holds `T` behind `Arc::from(box)`, downcast by `Arc::downcast::<T>`. [D R5, C]
- Lazy loads keep base ids; new modules are `base.modules.len()..` in collection order;
  `LazyWiring::singletons` lists the new singletons in connect order. [D R6, C]
- An execution counts in the live set from `LiveSet::enter`, before the phase check; `cancel_all`
  marks the set so a later attach is cancelled at once; `LiveSet::until_empty` resolves at once on
  an empty set. `SingletonStore::remove`, `AppShared::{get_root, find_module}` are `pub(crate)`.
  [D additions, E R4]
- The core's `Timer` module is `ModuleId(0)`, before the root's subtree. [C 17]
- `handle::sealed::State::write(record, bound)` replaces `bound(record) -> &mut Bound`, since the
  hook and readiness items live in an `Option` and a `Vec`. No public signature changed. [B,
  changes]
- `Key::from_parts` exists for a module hook's key (section 6). [E R1]
- Macro internals [G, changes]: `__private::{field, param}` replace `assert_site`, declaring and
  asserting `Site + AllowedIn<Sc>` in one call, a tuple field as `field("<index>")`;
  `__private::{assert_guard, assert_interceptor, assert_error_handler}` removed in favour of a
  local fn per entry; `IntoConstructed` carries an `on_unimplemented` naming the two constructor
  return shapes; `__private::factory`'s probes are `#[track_caller]` so a binding records the
  providers entry's line; `construct_span` spans the generated `construct` at the constructor;
  generated parameters are mixed-site, so a constructor parameter named `r` cannot shadow the
  resolver; `routes::Handler::method_tier` and `shared::attrs::has` removed as unread.

---

## 6. Applied by the orchestrating session

Two requests reached areas that had already finished; the orchestrating session applied them.

- **`Construct` carries an `on_unimplemented` hint.** [G R1, G 1] Applied text in
  `crates/ulo/src/construct.rs`: message "`{Self}` is not a type the container can construct",
  label "the container cannot build this", note "add #[injectable] to the type, or bind it with a
  factory". G's requested text differed (message "is not a type the container builds", label "this
  is bound with `provide`, which builds it through `Construct`", note ending with the example
  `m.singleton(|..| async { .. })`). The wording is unverified until the first compile.
- **`Key::from_parts`.** [E R1] `pub(crate) fn Key::from_parts(ty: TypeId, ty_name: &'static str,
  qualifier: TypeId, q_name: &'static str) -> Key` in `crates/ulo/src/key.rs`, the argument order
  both callers use (`graph/mod.rs` `find_module`, `lifecycle/connect.rs` `site_key`). It lets a
  module hook's failure name its module as a key (E 2).

---

## Wave 2

Wave 2 built D1–D20 as the thirteenth response signed them, the rename of the `Site` family
included. Its logs are `divergences/rename.md`, `wave2-W.md`, `wave2-R.md` and `wave2-M.md`.
Citations: `[W 3]` is entry 3 of `wave2-W.md`, `[W W1]` its request W1, `[R 2]` and `[M 4]`
likewise, `[rename 2]` entry 2 of `rename.md`; `[design-fold]` is a choice made while correcting
`DESIGN.md` for D1–D20, and `[orchestrator]` a name the orchestrating session fixed so the agents
and the design would agree. Nothing here has compiled yet.

Section 1 needs an answer per item, by number. Sections 2 to 6 need a read.

### 1. Decisions for you

**D21. A `#[module]` spelling for an enhancer contribution by value or by factory.** [M 4;
orchestrator]
§4's `into dyn Plugin: [MetricsPlugin, TracingPlugin]` lists types; §7 writes a global interceptor
by value against the value API and gives it no `#[module]` form. Built: an `into` list takes types
only, and a module needing a value or factory contribution implements `Module` by hand.
Options: (a) the providers-list rule inside `into` lists: a bare path is a type, anything else an
expression lowered to `.value(expr)`, which takes `Arc<K>`, so the user writes `Arc::new(..)`;
(b) the enhancer-attribute grammar, `into AnyInterceptor<Rpc>: [value = Tracing::default()]`, the
macro writing the `Arc::new`, with `with = closure` from the same grammar for a factory; (c) keep
the hand-written `Module`.
Recommendation: (b). It is the grammar `#[guards]` already uses for a value and a closure, and (a)
has no factory form.

**D22. A qualified enhancer contribution is accepted, read by no transport and reported by
nothing.** [W 3; orchestrator]
`m.enhancer::<AnyGuard<Http>>().qualified::<Q>()` compiles, because `enhancer` returns the builder
`contribute` returns, and registers under `AnyGuard<Http> @ Q`. `dispatch` walks
`entries::<AnyGuard<T>>()`, which takes no qualifier.
Options: (a) refuse at `wire()`: a record marked as an enhancer whose key carries a qualifier;
(b) a builder type for `enhancer` without `qualified`, giving up the shared builder; (c) accept
and document.
Recommendation: (a). The record carries both the enhancer mark and the qualifier, and §3.9's rule
refuses a written thing that does nothing where `wire()` can see it.

**D23. An `into` list tells a role key by how it is written, and an alias of one lowers to
`contribute`.** [M 2]
A proc macro sees tokens. `K` in `into K: [..]` is a role key when its last segment is `AnyGuard`,
`AnyInterceptor` or `AnyErrorHandler`, or when it is a `dyn` type with an `ErasedGuard`,
`ErasedInterceptor` or `ErasedErrorHandler` bound. `type HttpGuards = AnyGuard<Http>` lowers to
`contribute`: its entries sit in the collection `dispatch` walks, the key being the same `TypeId`,
and are scoped as providers, so an `Auto` guard among them that needs an execution is refused at
`wire()` as a singleton provider with §6.2's hint, which does not name the cause.
Options: (a) accept and document; (b) refuse at `wire()` every provider contribution under a key a
mounted handler walks as a role; (c) accept, and add "contribute it through `enhancer`" to the
`ScopeViolation` report when the refused binding's key is a mounted handler's role key.
Recommendation: (c). The alias form works for a singleton guard, and (b) would refuse working
code; (c) names the fix in the one report where the difference shows.

**D24. `override_many` answers `TestApp<PendingMany>`.** [W 6; orchestrator]
§11: `.qualified::<Q>()` is one spelling for every override kind. Built: `override_many` returns a
new public state, `TestApp<PendingMany>`, whose `qualified::<Q>()` answers `TestApp<Settled>` and
targets the collection `T @ Q`; it has no `in_module*`, a collection being app-wide. Every
`impl<S>` method stays reachable, so `override_many(..).connect()` reads as before; only code
naming the return type as `TestApp<Settled>` changes.
Options: accept; keep `Settled` and give a collection override no qualifier.
Recommendation: accept. `Many<T, Q>` exists, and the state is the device D5's sign-off chose.

**D25. A second `replace_module` of an original already replaced is dropped without a report.**
[W 5]
The registration walk takes the first replacement by identity, and the second is applied nowhere.
`ReplacementUnmatched` reports only an original no module imports.
Options: report it, naming both calls the way `DuplicateReadiness` names both locations; leave it.
Recommendation: report it. A replacement applied to nothing is the fault D4 was signed to report.

**D26. `.backoff(..)` as `#[track_caller]`.** [W W1; orchestrator]
`BackoffWithoutTimer::at` is the `.ready(..)` call: the readiness record keeps no location for
`.backoff`. R has finished; the change is one attribute on `backoff` and a `backoff_location` on
`ReadyRecord`, which W then reads.
Options: add it; keep `at` on `.ready(..)`.
Recommendation: add it. The line a report names is the line to edit, and `.ready(..)` is not it.

**D27. An interceptor sees a handler's panic before the error handlers do.** [R 2]
§7 step 4: `dispatch` catches a panic in a guard, an interceptor, the handler or an error handler
and offers the error handlers `PanicRecovered`; silent on the interceptors around a panicking
handler. Built: the handler and each interceptor run under their own catch inside `Next::run`.
The innermost interceptor's `next.run()` returns `Err(PanicRecovered { stage: Handler, .. })`,
each interceptor further out receives what the one inside it answered, and an interceptor can
reshape or answer a handler's panic as it can any handler error.
Options: keep; catch in `dispatch` alone, which unwinds the interceptors.
Recommendation: keep. A panic unwinding through the interceptors drops each mid-await, loses which
stage panicked, and takes the failed call from a timing interceptor.

**D28. What `ShutdownFailure::Close` holds for a timeout.** [R 5]
§9.5 and §10.2: a `close` exceeding its bound is dropped and recorded as `Close { transport,
source: Redacted }`; silent on what `source` holds then. Built: the redacted
`FailureReason::TimedOut { after, limit }`, `limit: ShutdownCap` with the cap's duration when the
cap bounded it, `Default` with `hook_timeout` otherwise. `FailureReason` gains `impl Error`, which
a `Redacted` needs to hold it, and a caller tells a timeout from the transport's own error by
`source.downcast_ref::<FailureReason>()`.
Options: (a) keep; (b) `Close { transport, reason: FailureReason }`, the shape `Hook` has, with
`Errored(Redacted)` holding the transport's error.
Recommendation: (b). `Hook` reports the same two outcomes through `reason`, and a match arm is
what the enum exists for; `NoTimer`'s downcast exists because `Bind`'s field must hold a foreign
error and the core's own in one type, which `FailureReason` already does.

---

### 2. What you will write differently

The rename:

- **`Construct::dependencies(d: &mut Dependencies)`**, with `d.field::<Dep<PgPool>>("pool")`,
  `d.param::<S>(name)` and `d.add::<S>()`; §13's hand-written `Construct` is written that way.
  `Factory`, `ShutdownFactory` and `Meta` declare through `dependencies` too, each a
  `dependencies(d: &mut Dependencies)`, `Meta`'s taking `&self`. [rename 1; design-fold]
- **`FromContainer::describe(req: &mut Requirement)`** keeps the by-reference shape; `req` is §3.2's
  spelling, `d` now being the `Dependencies` parameter. [rename 3; design-fold]
- **`ConstructError::Dependency`** replaces `ConstructError::Site`; a `match` on the variant
  changes. [rename 2]

Roles and contributions:

- **`m.enhancer::<AnyGuard<Http>>()`** is the one spelling for a global enhancer. It returns
  `Contribute<'_, AnyGuard<Http>, ()>`, the builder `contribute` returns, every method reachable,
  `provide` and `value` included. A `contribute` under a role key is a provider contribution: a
  negative bound cannot refuse it. [design-fold; W 1; M R3]
  `m.enhancer::<AnyGuard<Http>>().provide::<AuthGuard>(|a| a)`
- **`Role: sealed::Sealed + Send + Sync + 'static`**, no items and no `?Sized` supertrait,
  implemented for `AnyGuard<T>`, `AnyInterceptor<T>` and `AnyErrorHandler<T>` for every
  `T: Transport`. §3.7 writes `Sealed + 'static`; the two supertraits let `enhancer<R: Role +
  ?Sized>` build the `Contribute`, whose impl needs `U: Send + Sync`, and every implementor already
  is through the erased traits. [W 1; design-fold]
- **In `#[module]`, `into K: [A, B]` lowers to `m.enhancer::<K>()`** when `K` is written like a role
  key (D23), to `m.contribute::<K>()` otherwise; the list takes types only (D21). [M 2, M 4]

The pipeline:

- **`PanicRecovered { stage: DispatchStage, message: Redacted }` and `DispatchStage { Guard,
  Interceptor, Handler, ErrorHandler }`**, both `#[non_exhaustive]`, the first named after the
  event ulo already has. `PanicRecovered` implements `Debug`, `Display` and `Error`; `DispatchStage`
  derives `Clone`, `Copy`, `PartialEq`, `Eq` and `Hash` beside `Debug` and `Display`. [orchestrator;
  design-fold; R 4]
- **`Next::run`'s doc states that a handler's panic arrives as the `Err` it returns** (D27), and
  `ErrorHandler`'s doc that a panic in a guard, an interceptor, the handler or an earlier error
  handler arrives as `PanicRecovered`. [R 2; R R4]

Enhancer attributes:

- **A controller-level `value = expr` is a compile error**, in both forms: `#[guards(value = ..)]`
  and `#[guards(http(value = ..))]` on the impl. Per method both stand. [M 1]
- **`X as AnyGuard<Http>` in a providers list is a compile error**; a global guard is
  `into AnyGuard<Http>: [X]`. [M 3]

Lookups and keys:

- **`LookupError::Ambiguous { key: KeyName, sources: Vec<ModuleName> }`** is what `ModuleRef::get`,
  `exec.get` in an execution opened on a module, and `by_key` answer for a key two visible modules
  bind; `AmbiguousModule { module, candidates }` is back to `app.module::<M>()` alone. [R 1;
  design-fold]
- **`{:#}` on `Key` and `KeyName` prints full paths** on both sides of `@`, as in
  `my_app::db::PgPool @ my_app::Replica`, the ` (collection)` suffix kept; `{}` is unchanged.
  `Debug` delegates to `Display` and passes the flag along, so `{:#?}` prints full paths too.
  [orchestrator; R 7]

Tests:

- **`.qualified::<Q>()` on `TestApp<Pending>`** requalifies the last override's key and returns
  `TestApp<Pending>`, so `.in_module*` and `.everywhere()` still follow; a second call replaces the
  first. [W 6]
  `.override_value::<PgPool>(fake).qualified::<Replica>().in_module::<DbModule>()`
- **`override_many(..)` returns `TestApp<PendingMany>`** (D24). [W 6]

Additive public impls:

- **`Requirement` and `Dependencies` are `Clone`.** [R 8]
- **`FailureReason` implements `Error`** (D28). [R 5]

---

### 3. Behaviour that differs or was filled in

Roles:

- **A role is decided at freeze.** `ModuleDef::controller` gives `Controller`, a contribution
  through `enhancer` gives `Enhancer`, anything else `Provider`; the one later move is a binding an
  `EnhancerSpec` names by type, from provider to enhancer. A lazy load re-runs that move over the
  base bindings. `override_many` keeps each controller and enhancer mark on its record when it
  removes contributions. [W 2]
- **The two limits of token-level recognition** (D23): an alias of a role key lowers to
  `contribute`; a type the user named `AnyGuard`, or `dyn ErasedGuard<Http> + Send`, which is a
  different type from the role key, lowers to `enhancer` and fails the `Role` bound at compile
  time. [M 2]
- **A qualified enhancer contribution** registers under `AnyGuard<Http> @ Q` and nothing reads it
  (D22). [W 3]

The pipeline:

- **Which panics are `PanicRecovered`.** A panic while the container builds an enhancer or the
  controller, a per-execution by-type guard for example, stays `LookupError::Construct { reason:
  Panicked(..) }`. A panic in a `with = |..| ..` closure is `PanicRecovered` with the stage of the
  enhancer it builds. A guard's or interceptor's synchronous code before its first await counts as
  that stage. [R 3]
- **A panicking error handler's panic goes to the handlers after it**; unclaimed, the transport
  renders its internal-error status. [design-fold]
- **A handler's panic passes back through the interceptors** (D27). [R 2]

Wiring:

- **`.backoff(..)` on a timerless app** is refused at step 6. A zero backoff is not reported: the
  record holds `Duration::ZERO` when nothing is written, and a zero wait waits for nothing. A
  non-zero backoff is refused even with zero retries, where it would never run. [W 7]
- **The closure scope check** (step 5) refuses a hook, readiness, module-hook or metadata closure
  reading `Ext`, `ExecutionRef`, an input or a per-execution key, one entry per offending injection
  point. A per-execution key is a binding passing an execution need upward: execution-scoped, or
  transient needing one; a collection is refused when any contribution does. `Option<S>` is
  refused too, because without an execution it propagates `ExecutionRequired` rather than answering
  `None`. A closure reading its own binding is left to `HooksOnPerExecution`. Enhancer closures are
  exempt. [W 8]
- **`ExportNotBound.imported`** is true when the module's table holds the key from an import or a
  global, as a binding or an ambiguous entry. [W 4]
- **`ReplacementUnmatched`** is one per `replace_module` whose original identity the registration
  walk never reached; a second replacement of an identity already replaced is applied nowhere
  (D25). [W 5]
- **The near-spelling hint also matches a bound key equal in its last path segments**, under the
  same qualifier: "missing `Config`" beside a bound `b::Config` names it, and both print in full.
  §10.1 step 3 names only the trailing `+ Send + Sync` case. [W 9]

Tests:

- **Override matching compares keys as registered, qualifier applied**, so a `.qualified::<Q>()`
  binding is reached. Inside a keyed module the binding's own key is unqualified, and
  `in_module_keyed::<M, Q>()` reaches it without `.qualified`. [W 6]

Lookups:

- **`Ambiguous.key` carries `BindingKind::Single`**: a collection read gathers from every module
  and meets no ambiguous entry. `Option<S>` propagates `Ambiguous`. [R 1]

Executions and shutdown:

- **`Execution::open` stores a deadline and enforces nothing**; the transport that opened the
  execution does. [design-fold]
- **The close bound is read as each `close` starts**, so the cap's remainder shrinks with every
  close before it; closes run in reverse bind order, one after another. A `close` is polled before
  its bound, and one completing in the poll the bound fires in counts as closed, the tie rule hooks
  use. Once `shutdown_timeout` has expired, a `close` not finished on its first poll is dropped and
  recorded as `ShutdownFailure::Close`. Without a `Timer` there is no bound and `close` runs
  unbounded, which cannot occur for a bound server: `listen()` refuses a transport on a timerless
  app. [R 6; design-fold]

---

### 4. Diagnostics and error text

Texts:

- **`Ambiguous`:** "`dyn UserRepo` is ambiguous between PersistenceModule, LegacyRepoModule", the
  form `AmbiguousModule` writes. [R 1]
- **`PanicRecovered`:** "the guard panicked: {message}", with `interceptor`, `handler` and `error
  handler` for the other stages. The message is `redact_panic`'s output, so registered secrets and
  URL userinfo are scrubbed from what it prints. [R 4]
- **`ShutdownFailure::Close` on a timeout:** "transport `Http` failed to close: timed out after 5s
  (`shutdown_timeout`)" (D28). `Limit`'s docs name the close beside the hook. [R 5]
- **The `Missing` help reads "the injection point reads"**, §10.1's sample. [rename 4; design-fold]
- **`#n` on the second and later module of one type and qualifier, labelled or not** (D12): a label
  names every configuration of an integration alike. [design-fold]

New `WiringError` variants:

- **`ExportNotBound { module, key, imported: bool, near: Option<KeyName>, at }`.** `near` is a key
  the module binds itself spelled like the export, and when present its hint wins: "{module} binds
  `dyn Repo + Send + Sync`; the export names `dyn Repo`". Otherwise "an import of {module} provides
  `{key}`; re-export it with `reexport`, or bind it in {module}", or "bind `{key}` in {module}, or
  remove the export". [W 4]
- **`ReplacementUnmatched { original: ModuleName, at }`**, `at` the `replace_module` call. [W 5]
- **`BackoffWithoutTimer { binding: String, at }`**, its own variant because `BoundWithoutTimer`'s
  help, "leave the bound at its default or write `.unbounded()`", does not apply to a backoff; `at`
  is the `.ready(..)` call (D26). [W 7]
- **`ClosureNeedsExecution { closure: String, path: Vec<String>, at: Option<Location> }`.** `closure`
  reads ``readiness check of `PgPool` in DbModule``, ``` `OnModuleDestroy` hook of `PgPool` in
  DbModule ```, ``` `OnModuleInit` hook of module UsersModule ``` or ``metadata `Middleware` of
  UsersModule``; a metadata value has no location. `path` runs from the injection point to the read
  of execution data: ``Dep<AuditContext> (param #1) → AuditContext (execution) → Ext<CurrentUser>
  (field `user`)``, or ``input `RequestHead` `` for an input. [W 8]
- **`OverrideUnmatched` gains `keyed: Option<ModuleName>`**: for a qualified override matching
  nothing, a keyed module under that qualifier binding the key unqualified is named, with the help
  "reach it with `.in_module_keyed::<M, Q>()` and no `.qualified`". [W 6]

Full paths on collision:

- **`WiringErrors`' `Display` gathers every `KeyName` across its entries**, groups them by short
  text without the ` (collection)` suffix, and prints each key in a group of two or more distinct
  keys with `{:#}`; a `WiringError` displayed alone applies the rule to its own keys. Limit: only
  `KeyName` fields are reformatted. The strings rendered before the report exists, `consumer`, path
  steps, `item`, `handler` and `closure`, and every `ModuleName`, stay short, so two module types
  sharing a last segment still print alike. [W 9; R 7]

Renamed texts [rename 4]:

| Where | Old | New |
|---|---|---|
| `FromContainer` `on_unimplemented` message | `` `{Self}` is not an injection site `` | `` `{Self}` cannot be obtained from the container `` |
| `AllowedIn` label | `this site needs an execution` | `this injection point needs an execution` |
| `Factory` message | `` `{Self}` is not a factory over sites `` | `` `{Self}` is not a factory the container can call `` |
| `Factory` and `ShutdownFactory` labels | `injection site(s)` | `injection point(s)` |
| `Factory` note | `annotate each parameter with its site type` | `annotate each parameter with its type` |
| macro: factory parameter without a type | `needs its site type written` | `needs its type written` |
| macro: constructor with `self` | `each an injection site` | `each an injection point` |
| macro: constructor parameter pattern | `print as the site's name` | `print as the parameter's name` |
| macro: `#[injectable(default)]` marker | `every other field is a site` | `every other field is injected` |
| macro: `#[injectable]` on another item | `whose fields are sites` | `whose fields are injected` |
| macro: `with = ..` not a closure | `whose parameters are sites` | `whose parameters are injection points` |

`FromContainer`'s second note is kept verbatim:
`or set it inside the #[construct] fn / mark the field #[injectable(default)]`. None of the text
suggests implementing the trait.

Compile errors:

- **A controller-level `value`:** "a `value` entry on the impl would be built once per handler
  rather than shared; declare it per method, or bind it by type for shared state", one error per
  entry, spanning from the entry's first token, the scope key or `value`, to the end of the
  expression; in the scoped form the closing parenthesis falls outside the span. [M 1]
- **`X as <role key>`:** on the role key, "a role key takes contributions, and `as` binds a single
  instance; a global enhancer is written `into AnyGuard<Http>: [AuthGuard]`". `as` lowers to
  `also_as`, a second single key, which `entries` never lists and which beside a contribution to the
  same key is a single/collection mix. [M 3]
- **`Role`'s `on_unimplemented`:** message "`{Self}` is not a role key", label "a global enhancer is
  contributed under a role key", note "the role keys are `AnyGuard<T>`, `AnyInterceptor<T>` and
  `AnyErrorHandler<T>` for a `T: Transport`". A type written like a role key that is not one fails
  there. [M R2; W 1]

---

### 5. Internal only, listed for completeness

- The enhancer mark: `ModuleDef::enhancer` returns the `Contribute` builder carrying a mark; each
  record it pushes is listed in `ModuleNode::enhancers`, and freezing gives those bindings
  `Role::Enhancer`. `scopes::assign_roles` no longer resets roles and reads no key name; C 6's
  `type_name` probe is replaced. [W 1, W 2]
- `Graph`, `FrozenModule`, `FrozenBinding`, `Edge`, `EdgeTarget`, `VisibilityTable`, `Visible`,
  `InputDecl`, `Override`, `OverrideTarget` and `CollectionOverride` derive `Clone`; `wire_lazy`
  calls `base.clone()`, every `clone_*` helper is gone, and override application clones `Recipe`
  and `Dependencies` directly. `TestPlan` does not derive: `Replacement` holds a `Box<dyn Module>`.
  [W 10; R R2]
- `BindingRecord`, `AlsoAs`, `Recipe`, `ReadyRecord`, `HookRecord`, `HandlerDecl`, `EnhancerDep`,
  `HandlerRecord`, `ControllerRecord`, `DependencyRecord`, `Read` and `ReadKind` derive `Clone`; the
  last three, `ControllerRecord`, `Qualifier` and `DependencyLabel` also `Copy`. Every closure field
  was already an `Arc`: no constructor changed, and a clone shares each closure and value with the
  original. [R 8]
- An override's key may carry a qualifier; the record an `also_as` override splits off, and each
  `override_many` item, keeps `primary` unqualified and the qualifier apart, as every other record
  does. [W 6]
- Role-key recognition looks through parentheses and the invisible group a `macro_rules` `$t:ty`
  produces. [M 2]
- Rename internals [rename 5, 6, 7]: `crates/ulo/src/site/` → `dependency/`,
  `ulo-macros/src/shared/sites.rs` → `shared/dependencies.rs`. `SiteRead` → `Read`, `SiteRecord` →
  `DependencyRecord` (`.desc` → `.requirement`), `SiteLabel` → `DependencyLabel`, every `sites`
  field → `dependencies`, `Edge.site` → `Edge.dependency`, `Steps.sites` → `Steps.dependencies`,
  the `site_*` and `*_sites` functions → `dependency_*` and `*_dependencies`, `sites_deps` →
  `closure_deps`; `HookSite`/`site_hooks`/`site_key` → `HookOwner`/`hooks_of`/`owner_key`, which were
  not part of the family; the `site: usize` parameters of `Graph::consumer` and
  `Graph::dependency_step` → `index`. Macros: `SiteSpec`/`SiteLabel` →
  `DependencySpec`/`DependencyLabel`, `sites_param()` and the generated `s` → `dependencies_param()`
  and `d`, `ConstructImpl.sites` → `.dependencies`, `FieldRole::Site` → `::Dependency`,
  `__ulo_site_{i}` → `__ulo_dep_{i}`. Kept: `Span::call_site`, `Span::mixed_site`, "mixed-site", "a
  factory's call site", "call-site span", `visited`.
- `BUILD_PLAN.md`'s area G row names `field` and `param` in place of `assert_site`, which
  `__private.rs` never defined. [rename 8]
- Doc comments: `AnyGuard` and `Contribute` show `m.enhancer::<AnyGuard<Http>>()`, and `Contribute`
  states that `contribute` records a provider contribution whatever its key. [W; M R1]
- Design text placed by the fold [design-fold]: the failed-`connect` limit, what a dropped app's
  init hooks did outside the process, sits in §9.2 only; the misspelled-transport-key limit (D6) in
  §7 only; §12's row is worded "A written wait"; `ConstructError::Site` is spelled nowhere in the
  design, which closes rename 2's open item.

---

### 6. Superseded wave 1 entries

- **C 3, C E1** (an unbound export reported as `Missing` with a consumer string) → `ExportNotBound`
  [W 4]; D3 as signed.
- **C 6** (role detection by `type_name` prefix through a probe transport) → `m.enhancer::<R:
  Role>()` and the role mark at freeze [W 1, W 2, M 2]; the thirteenth response's first added
  decision.
- **C 7**, its statement that closure sites are checked for keys and not scope →
  `ClosureNeedsExecution` [W 8]; D10 as signed.
- **C 14, C E2** (an unmatched `replace_module` unreported) → `ReplacementUnmatched` [W 5]; D4 as
  signed.
- **D 6** (no override spelling reaches a `.qualified::<Q>()` binding) → `.qualified::<Q>()` on
  `TestApp<Pending>` and `TestApp<PendingMany>` [W 6]; D5 as adjusted.
- **E 5** (`.backoff` on a timerless app accepted) → `BackoffWithoutTimer` [W 7]; D8 as signed.
- **A 4, E 11's `AmbiguousModule` reuse** ("`X` is ambiguous between A, B" reading for both) →
  `LookupError::Ambiguous` carries that text and `AmbiguousModule` names a module type alone [R 1];
  D2 as signed.
- **F 4** (a pipeline panic unwinds out of `dispatch`) → caught per boundary, `PanicRecovered {
  stage, message }` [R 2, R 3, R 4]; D14 as signed.
- **E 12**, "`close` runs per transport in reverse bind order, one after another, unbounded" →
  bounded by the cap's remainder or `hook_timeout`, a timeout recorded as `ShutdownFailure::Close`
  [R 5, R 6]; the thirteenth response's third added decision.
- **A 1's cost**, "`a::Config` and `b::Config` print alike in every diagnostic" → full paths on
  collision for keys [W 9, R 7]; the second added decision. Module names still print short.
- **G 3** (a controller-level `value = expr` built once per handler) → a compile error [M 1]; D7 as
  adjusted.
- **D20's `clone_graph`** and its hand copies → derived `Clone` [W 10, R 8].
- **design-notes 1, spine 23** (`Sites::{field, param, site}`, `SiteDesc`) → `Dependencies::{field,
  param, add}`, `Requirement` [rename]; D17 replaced by the rename.
- **spine 8** (`Factory<Args>` and `ShutdownFactory<Args>` over closures whose parameters are each
  a `Site`) and **spine 16** (`Meta::sites`) → `FromContainer`, `Factory::dependencies`,
  `ShutdownFactory::dependencies`, `Meta::dependencies` [rename 1].
- **B 7** (`ConstructError`'s `Debug` writing `Site(..)`) → `Dependency(..)` [rename 2].
- **G 9** (a non-`Site` field or parameter reporting with "the `Site` message") →
  `FromContainer`'s text, "`{Self}` cannot be obtained from the container" [rename 4].
- **spine 32** (`override_many(..)` answering `TestApp<Settled>`) → `TestApp<PendingMany>` [W 6];
  D24.
- **Wave 1 §5's** "`__private::{field, param}` replace `assert_site`, declaring and asserting
  `Site + AllowedIn<Sc>`" → the same two functions asserting `FromContainer + AllowedIn` [rename 8];
  **wave 1 §6's** `lifecycle/connect.rs` `site_key` is `owner_key` [rename 6].
- The word "site" in D1, D10, D15 and §3's factory entry reads "dependency" or "injection point";
  the decisions stand. [rename]

---

## Wave 3

Wave 3 built D21–D28 as the fourteenth response signed them. Its logs are `divergences/wave3-W.md`,
`wave3-R.md` and `wave3-M.md`. Citations: `[W3 2]` is entry 2 of `wave3-W.md`, `[W3 L1]` its
request L1, `[R3 4]` and `[M3 5]` likewise, `[M3 R1]` M's request R1. Nothing here has compiled;
R's entries 1, 3 and 5 rest on two stub probes compiled with `rustc +1.88 --edition 2024`.

The four items the wave left open were decided on 2026-10-03 and are in `DESIGN.md`. Section 1
records them; section 2 needs a read.

### 1. Decisions made

**D29. Two dead forms under a role key are refused at `wire()`.** [W3 3; M3 5; M3 R1]
With roles given by `TypeId` at freeze, `m.contribute::<AnyGuard<Http>>().qualified::<Q>()`
compiled, took the enhancer role and was read by no transport, D22's fault through the other
builder. A single binding under a role key, `X as HttpGuards` through `also_as`, an alias, or the
value API's `provide`/`value`, was bound and guarded nothing, reported only when a contribution to
the same key made it a single/collection mix. Decided: refuse both in the freeze pass that assigns
roles, a qualified contribution with "contribute it unqualified" and a single binding with
"contribute it". A role key no mounted handler reads and no `enhancer` marks is outside the set
and not checked. `#[module]`'s compile error on `X as <role key>` stays in front of it.

**D30. `Contribute::try_value`.** [M3 3; M3 R2]
`value = expr?` in an `into` list was a span error, "a contribution has no fallible value form":
`Contribute` had no `try_value`, and a lowered `?` would sit in `register`, which returns `()`.
Decided: add `try_value<E: Into<BoxError>>(self, value: Result<Arc<U>, E>)`, recording an `Err`
for `wire()` as `ModuleDef::try_value` does, under the collection key. `value = expr?` lowers to it
and reports at `wire()` like a providers list's `expr?`. M's limit on a configured module's
fallible value built from its own fields, which a `'static` `with` closure cannot reach, goes with
the refusal.

**D31. A `#[module]` spelling for a role key no mounted handler reads.** [M3 6]
An `into` list lowers to `contribute` and cannot mark, so a module contributing enhancers for a
transport the app mounts no handler of, a shared auth module in a worker binary or a test app
without controllers, is refused at `wire()` when one of its `Auto` enhancers needs an execution.
M offered `into enhancer K: [..]`, lowering to `m.enhancer::<K>()`. Decided: deferred, stated as a
known limit in §7. One hand-written `m.enhancer::<K>()` anywhere in the graph covers it, since one
marked contribution marks the whole key.

**D32. `is_panic` as built.** [R3 3; R3 4; R3 5]
`pub fn is_panic(error: &BoxError) -> bool`, re-exported at the crate root. It answers `true` for
`PanicRecovered`, `LookupError::Construct { reason: Panicked }` and a `ConnectError` whose reason
is `Panicked`, bare or inside `LoadError` or `StartupError`; follows `Errored`'s `Redacted`,
`ConstructError::Failed` and `source()` chains to 32 levels; and inside a `Redacted` recognises
the core's own types alone, since `Redacted` hands out its original by type. `&dyn Error` was
probed on 1.88 and failed E0277 for `is_panic(&err)`, the coercion unsizing the `Box` itself.
Decided: as built. §10.2's "by downcast of the error itself; no `source()` chain is walked" is
replaced.

### 2. For reading

Roles and contributions:

- **`Plain` and `Enhancer` are empty enums**, the device `Open`, `Set`, `Pending` and `Wired` use.
  `Contribute` keeps a private `enhancer: bool` written by each mark's constructor, so the shared
  `impl<'m, U, Q, M>` carries no bound on `M`; `m.enhancer::<AnyGuard<Http>>().qualified::<Q>()`
  is E0599 at `.qualified`. [W3 1]
- **Exported at `ulo::handle`**, beside `Handle`'s state markers, that module's doc widened; a bare
  `ulo::Enhancer` at the root would read like a trait. [R3 8; W3 L1]
- **`scopes::mark_role_contributions` runs at the end of `freeze`**, once every controller has
  mounted. The set is the `role_keys` of every handler in the graph, a lazy load's base included,
  plus the primary key of every collection binding marked `Enhancer` from `ModuleNode::enhancers`;
  every collection binding under a key in the set becomes an enhancer. `assign_roles`, which needs
  the visibility tables, still runs in `check`. [W3 2]
- **The `X as <role key>` compile error stays**, text unchanged; `written_as_role_key` serves that
  diagnostic alone, and no lowering depends on it. Its false refusal: a user's own type named
  `AnyGuard`, `AnyInterceptor` or `AnyErrorHandler`, or a `dyn` of a trait named `ErasedGuard`,
  `ErasedInterceptor` or `ErasedErrorHandler`, is refused after `as`; renaming the import binds
  it. [M3 5]

The `into` grammar:

- **A `with` closure in an `into` list is wrapped as `#[guards]` wraps it**: a synchronous body in
  `async move`, an `async` closure or block kept. Limit: a synchronous body returning a future it
  does not await, `|c: Dep<C>| Plugin::connect(c)` with `connect` an `async fn`, is a future of a
  future and fails at the coercion `|a| a`; `#[guards]` has the same limit. [M3 1]
- **`with` builds a singleton in an `into` list and per execution in `#[guards]`**: a closure
  reading `Ext<CurrentUser>` works on a handler and is refused at `wire()` in an `into` list as a
  singleton needing an execution, §6.2's report naming the scope and not the attribute the
  closure was copied from; a per-execution contribution is `.execution(..)` or
  `.try_execution(..)` in a hand-written `Module`. [M3 2]
- **A malformed `into` item** is a span error naming the three forms, with "; an expression is
  contributed with `value = ..`" when the item parses as one: a call such as
  `MetricsPlugin::new()`, which wave 2 turned into `provide::<MetricsPlugin::new()>`, a
  transport-scoped form, or a key before `=` other than `value` and `with`; a non-closure `with`
  and an untyped closure parameter have their own texts. [M3 4]
- **What the expansion names:** `m.contribute::<K>()` with `provide::<A>(|a| a)` and
  `value(Arc<K>)` on the `Plain` builder, `::ulo::__private::Arc`, and
  `::ulo::__private::factory::{Probe, FallibleContribution, PlainContribution}`, whose impls call
  `Contribute::try_singleton::<Args, F, T, E>` and `Contribute::singleton::<Args, F>` by
  turbofish, so the order of those generic parameters is part of the contract. [M3]

Replacements and readiness:

- **`WiringError::DuplicateReplacement { original: ModuleName, first, second }`**, step 2, the two
  `replace_module` locations; text "two `replace_module` calls replace {original}", "help: keep
  one; wiring applied only the first". Detected from the test plan's replacements in order; over an
  original nothing imports, the first call is `ReplacementUnmatched` and each later one
  `DuplicateReplacement`, never both on one call. [W3 4]
- **`.backoff(..)` records its location**, `backoff_location: Option<&'static Location<'static>>`,
  each call overwriting the duration and the location together, a replacing `.ready(..)` starting
  a new record with `None`; `.backoff(5s)` then `.backoff(Duration::ZERO)` reports nothing, since
  `wire()` tests the final duration. `BackoffWithoutTimer::at` is that location, its fallback to
  the `.ready(..)` location unreachable while the check fires only on a non-zero backoff. [R3 6;
  W3 5]

Shutdown and errors:

- **A panicking transport `close` is caught** and recorded as `Close { transport, reason:
  Panicked }`, redacted by `redact_panic`; `close` is called inside the caught future, so a
  `Server::close` panicking before returning its future is caught too. Before this it unwound out
  of the shutdown sequence into whichever `serve` or `close` call was running it. A `close`
  exceeding its bound is `TimedOut`, `ShutdownCap` when the cap bounded it, the first-poll case
  after expiry included, `Default` with `hook_timeout` otherwise; `Skipped` never occurs on
  `Close`. Text: "transport `Http` failed to close: panicked: " and the redacted message. [R3 1]
- **`FailureReason` no longer implements `Error`**; `Display` and `Debug` stay. Nothing boxes one
  now that `Close` holds the reason as a field, and no code under `crates/` used the impl.
  `BoxError::from(reason)` and `?` into a `BoxError` do not compile; hold it as a field, as `Hook`,
  `Construct` and `Close` do. [R3 2; R3 R3]
- **D27's sentence sits on `Next::run`'s doc**, with its warning, stated once on the method that
  returns the `Err`. `Server::close`'s doc names the three reasons and `transport/pipeline.rs`'s
  module doc points to `ulo::is_panic`; R wrote both, no agent owning those files this wave.
  [R3 9; R3 R2]

Full paths on collision:

- **`ModuleName` holds both texts.** `{:#}` writes the type's and the qualifier's full paths with
  the `#n` suffix, `my_app::db::DbModule @ my_app::Replica #2`; a labelled module keeps its label
  in both forms and `{:#}` follows it with the type's path, `redis (my_app::Redis) @
  my_app::Primary`. `Display` writes through `Formatter::pad`, `Debug` passes the flag along, and
  equality and hashing cover both texts. `module::colliding_names` groups by short text;
  `WiringErrors`' `Display` runs it over every `ModuleName` field, a `WiringError` displayed alone
  over its own, and the `Ambiguous` exporter column is padded to the widest name as printed.
  Limits: `OverrideModuleAmbiguous::module` is a `&'static str` and stays short, its candidates
  sharing one type; `consumer`, path steps, `item`, `handler` and `closure` stay short. [W3 6]
- **`LookupError::Ambiguous` and `AmbiguousModule` take the rule** through `write_list`:
  `billing::Module` and `users::Module` print apart, and two candidates whose qualifiers share a
  last segment, `DbModule @ Replica` from `a::Replica` and from `b::Replica`, are written apart.
  [R3 7; W3 R1]

Doc comments: `Contribute`, `ModuleDef::contribute`, `ModuleDef::enhancer`, `Role`, the
`transport` module, `graph::Role`, `assign_roles` and `freeze` state the `TypeId` rule;
`enhancer`'s names its remaining job, marking a role key no mounted handler reads;
`TestApp::replace_module` names `DuplicateReplacement`, and `BackoffWithoutTimer` names the
`.backoff(..)` call as `at`. [W3 7]

Superseded wave 2 entries:

- **W 3** (a qualified enhancer contribution accepted and unreported) → E0599 on the `Enhancer`
  builder [W3 1], and the `contribute` form a wiring error (D29).
- **W 2's** "a contribution through `contribute` under a role key stays a provider", **M 2**,
  **§2's** "`into K: [A, B]` lowers to `m.enhancer::<K>()` when `K` is written like a role key",
  **§3's** two limits of token-level recognition and **§5's** look-through → every `into` list
  lowers to `contribute` and the core gives the role by `TypeId` at freeze [W3 2; M3]; the
  look-through survives for the `as` diagnostic and the `into` item's call check alone [M3 5].
- **M 3** stands under D23 [M3 5]. **M 4** (types only) → `value = expr`, `with = closure` (D21)
  and `value = expr?` (D30). **M R1, M R2, M R3** → applied in `binding/contribute.rs`; the `Role`
  text serves a hand-written `enhancer` call; the expansion names no `enhancer` [M3].
- **W 5** (a second replacement unreported) → `DuplicateReplacement` [W3 4]; D25 as signed.
  **W 7's** `at` on the `.ready(..)` call → the `.backoff(..)` call [W3 5; R3 6]; D26 as signed.
- **W 9's** limit that `ModuleName`s print short, **§4's** "every `ModuleName` stay short" and
  **§6's** "Module names still print short" → full paths on collision for module names [W3 6;
  R3 7].
- **R 5** (`FailureReason` implements `Error`) → removed [R3 2]; D28 as (b). **§3's** close-bound
  entry, silent on a panic → caught and `Panicked` [R3 1].
