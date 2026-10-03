# Divergences: wave 2, agent W (the graph, modules, wiring report, test builder, roles)

Every place W's code departs from `DESIGN.md`, or fills a gap the design leaves that a user would
see. Each entry gives what the design says, what W wrote, and why. Requests for other agents' files
follow the entries.

Files: `graph/{mod,wire,scopes,visibility}.rs`, `module/def.rs`, `error/wiring.rs`, `testing.rs`,
`transport/mod.rs`, `binding/contribute.rs`.

Superseded by this wave: C 3 (unbound export reported as `Missing`), C 6 (role detection by
`type_name` prefix), C 7's statement that closure sites are not scope-checked, C 14's unreported
unmatched replacement, C's requests E1 and E2, D 6 (no override reaches a qualified binding) and
E 5 (`.backoff` without a `Timer` accepted).

## Entries

### 1. What `Role` carries

- **Design:** §3.7: `pub trait Role: sealed::Sealed + 'static {}`. The brief left what it carries
  for the graph to W.
- **W:** `pub trait Role: sealed::Sealed + Send + Sync + 'static {}` with no items, implemented for
  `AnyGuard<T>`, `AnyInterceptor<T>` and `AnyErrorHandler<T>` for every `T: Transport`, and carrying
  M's `#[diagnostic::on_unimplemented]` ("`{Self}` is not a role key", listing the three keys). The
  graph learns the role from the call: `ModuleDef::enhancer` returns the `Contribute` builder marked
  so each record it pushes is listed in `ModuleNode::enhancers`, and freezing gives those bindings
  `Role::Enhancer`. The kind and the transport are the key's own type, which the record already
  carries as a `TypeId`; no check reads them apart from the key, so the trait adds nothing for a
  compiler to change.
- **Why:** `Send + Sync` supertraits let `enhancer<R: Role + ?Sized>` build `Contribute<'_, R, ()>`,
  whose impl needs `U: Send + Sync`; every implementor already is, through the erased traits'
  supertraits. A hidden const or fn that no pass reads would be dead weight.

### 2. Roles are decided at freeze

- **Design:** §7, §14.18: the entry decides the role; a contribution through `contribute` is a
  provider contribution whatever its key.
- **W:** freezing sets `Controller` for `ModuleDef::controller`, `Enhancer` for a contribution
  through `enhancer`, and `Provider` otherwise. `scopes::assign_roles` no longer resets roles and
  reads no key name; it only moves a binding an `EnhancerSpec` names by type from provider to
  enhancer. A lazy load re-runs it over base bindings, whose roles can only move that way.
  `override_many` keeps each controller and enhancer mark on its own record when it removes
  contributions.
- **Why:** the role is a fact of registration, and the graph's records already preserve it.

### 3. A qualified enhancer contribution

- **Design:** silent; `Resolver::entries` takes no qualifier, as role collections are unqualified
  (§3.2).
- **W:** `m.enhancer::<AnyGuard<Http>>().qualified::<Q>()` compiles, since the builder is the one
  `contribute` returns, and registers enhancers under `AnyGuard<Http> @ Q`. No transport reads that
  collection and nothing reports it.
- **Why:** open. Refusing it needs either a builder type without `qualified` or a wiring error;
  neither is in the design.

### 4. `ExportNotBound`

- **Design:** §10.1 step 1: `ExportNotBound`, with the hint to bind it there, or to re-export it
  with `reexport` when an import provides it.
- **W:** `ExportNotBound { module, key, imported: bool, near: Option<KeyName>, at }`. `imported`
  is true when the module's table holds the key from an import or a global (a binding or an
  ambiguous entry). `near` is a key the module binds itself spelled like the export (entry 9's
  rule), and when present its hint wins: "{module} binds `dyn Repo + Send + Sync`; the export names
  `dyn Repo`". Otherwise the help is "an import of {module} provides `{key}`; re-export it with
  `reexport`, or bind it in {module}", or "bind `{key}` in {module}, or remove the export".
- **Why:** the `Send + Sync` mismatch C used to pass through `Missing::near` is as likely in an
  export list as at an injection point.

### 5. `ReplacementUnmatched`

- **Design:** §10.1 step 2, §11: a `replace_module` whose original no module imports.
- **W:** `ReplacementUnmatched { original: ModuleName, at }`, one per `replace_module` whose
  original identity the registration walk never reached; `at` is the `replace_module` call. A
  second `replace_module` of an identity already replaced is applied nowhere, since the walk takes
  the first, and is not reported.
- **Why:** the duplicate is matched by identity, and the design names only the unmatched case.

### 6. The override qualifier

- **Design:** §11: `.qualified::<Q>()` on the pending override, one spelling for every override
  kind, in the order the replaced binding was written.
- **W:**
  - `TestApp<Pending>::qualified::<Q>() -> TestApp<Pending>`: it requalifies the last override's
    key, so `.in_module*` and `.everywhere()` still follow; a second call replaces the first.
  - `override_many` now returns `TestApp<PendingMany>`, a new public state marker in
    `ulo::testing`, whose `qualified::<Q>() -> TestApp<Settled>` targets the collection `T @ Q`.
    `PendingMany` has no `in_module*`: a collection is app-wide. Every `impl<S>` method stays
    reachable, so `override_many(..).connect()` reads as before; only code naming the return type
    as `TestApp<Settled>` changes.
  - Matching compares keys as registered, qualifier applied, so a `.qualified::<Q>()` binding is
    reached. Inside a keyed module the binding's own key is unqualified, and
    `in_module_keyed::<M, Q>()` reaches it without `.qualified`.
  - `OverrideUnmatched` gains `keyed: Option<ModuleName>`: for a qualified override that matches
    nothing, a keyed module under that qualifier binding the key unqualified is named, with the
    help "reach it with `.in_module_keyed::<M, Q>()` and no `.qualified`".
  - An override's key may now carry a qualifier, so the record an `also_as` override splits off,
    and each `override_many` item, keeps its `primary` unqualified and the qualifier apart, as
    every other record does.
- **Why:** `override_many` is an override kind, and `Many<T, Q>` exists; a separate state keeps
  `in_module` off a collection at compile time. The `keyed` hint covers the one place where the two
  qualifier spellings meet and the plain "matches no binding" would mislead.

### 7. `.backoff(..)` on an app with no `Timer`

- **Design:** §3.9, §9.3, §10.1 step 6, §14.21: a wiring error like any written bound.
- **W:** `BackoffWithoutTimer { binding: String, at }`, step 6. `at` is the `.ready(..)` call: the
  readiness record keeps no location for `.backoff`. A zero backoff is not reported: the record
  holds `Duration::ZERO` when nothing is written, and a zero wait waits for nothing. A non-zero
  backoff is refused even with zero retries, where it would never run.
- **Why:** its own variant, because `BoundWithoutTimer`'s help ("leave the bound at its default or
  write `.unbounded()`") does not apply to a backoff.

### 8. The closure scope check

- **Design:** §6.2, §10.1 step 5: refuse a hook, readiness, module-hook or metadata closure that
  reads `Ext`, `ExecutionRef`, an input or a per-execution key; enhancer closures are exempt.
- **W:** `ClosureNeedsExecution { closure: String, path: Vec<String>, at: Option<Location> }`,
  step 5, one entry per offending injection point. `closure` reads ``readiness check of `PgPool` in
  DbModule``, ``` `OnModuleDestroy` hook of `PgPool` in DbModule ```, ``` `OnModuleInit` hook of
  module UsersModule ```, or ``metadata `Middleware` of UsersModule``; a metadata value has no
  location. `path` starts at the injection point and runs to the read of execution data:
  ``Dep<AuditContext> (param #1) → AuditContext (execution) → Ext<CurrentUser> (field `user`)``,
  or `input `RequestHead`` for an input. Rules:
  - A per-execution key is a binding that passes an execution need upward: execution-scoped, or
    transient needing one. A collection is refused when any contribution does.
  - Optional reads are refused too: with no execution, `Option<S>` propagates `ExecutionRequired`
    rather than answering `None`.
  - A closure reading its own binding is left to `HooksOnPerExecution`.
- **Why:** these are the reads that fail at runtime wherever such a closure runs. Reporting the
  self-read as well would report one fault twice.

### 9. Full paths on collision

- **Design:** §10.1, §14.19: short names by default; where one report would print two different
  keys alike, those keys print with their full paths.
- **W:**
  - `WiringErrors`' `Display` gathers every `KeyName` across all its entries, groups them by short
    text without the ` (collection)` suffix, and prints each key in a group of two or more distinct
    keys with `{:#}`. A `WiringError` displayed alone applies the rule to its own keys.
  - The near-spelling hint on a missing dependency now also matches a bound key equal in its last
    path segments, under the same qualifier. So "missing `Config`" beside a bound `b::Config` names
    it, and both print in full.
  - **Limits:** only `KeyName` fields are reformatted. The strings the graph renders before the
    report exists (`consumer`, path steps, `item`, `handler`, `closure`) and `ModuleName`s stay
    short, so two module types sharing a last segment still print alike.
- **Why:** the check runs at formatting time over what the report holds as keys. Re-rendering the
  pre-built strings would mean carrying keys inside every path step.

### 10. `clone_graph`

- **Design:** D20's outcome: replace the hand-written copies with `.clone()`.
- **W:** `Graph`, `FrozenModule`, `FrozenBinding`, `Edge`, `EdgeTarget`, `VisibilityTable`,
  `Visible` and `InputDecl` derive `Clone`, and `wire_lazy` calls `base.clone()`. Every
  `clone_*` helper is gone, and override application clones `Recipe` and `Dependencies` directly.
  `Override`, `OverrideTarget` and `CollectionOverride` derive `Clone`. `TestPlan` does not:
  `Replacement` holds a `Box<dyn Module>`.
- **Why:** R made every record the graph copies `Clone` (R's request R2).

## Requests from other agents, honoured

- **M R1:** the doc comments on `AnyGuard` (`transport/mod.rs`) and on `Contribute`
  (`binding/contribute.rs`) now show `m.enhancer::<AnyGuard<Http>>()`. `Contribute` also states
  that `contribute` records a provider contribution whatever its key.
- **M R2:** the diagnostic is on `Role` as suggested (entry 1).
- **M R3:** `ModuleDef::enhancer<R: Role + ?Sized>(&mut self) -> Contribute<'_, R, ()>`. Every
  `Contribute` method, `provide` included, is reachable for `R = AnyGuard<T>`.
- **R R1:** `pub trait Role` is defined in `transport/mod.rs`.
- **R R2:** entry 10.
- **R R3:** the report uses `{:#}` (entry 9).
- **R R4:** `ErrorHandler`'s doc adds that a panic in a guard, an interceptor, the handler or an
  earlier error handler arrives as `PanicRecovered`.

## Requests for other agents

### W1 (R, optional). A location for `.backoff`

- **File, item:** `binding/mod.rs`, `ReadyRecord`; `binding/handle.rs`, `backoff`.
- **Request:** `backoff` as `#[track_caller]` writing a `backoff_location`, so
  `BackoffWithoutTimer::at` points at the `.backoff(..)` call rather than at `.ready(..)`. W
  switches to it once it exists.
