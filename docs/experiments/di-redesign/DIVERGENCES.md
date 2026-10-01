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
