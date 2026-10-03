# Divergences: wave 6, core (scope as its own axis)

Every place the core's code departs from the brief, or fills a gap a user would see. Each entry
gives what the brief says, what the core wrote, and why. Requests for other agents' files follow
the entries.

The decision is RESPONSE.md's sixteenth response: `with = closure` means only "built by this
closure". Its scope follows a type's rule: `Auto` by default, and `singleton`, `execution` or
`transient` override it. It replaces wave 5's role rule (`divergences/wave5-core.md`).

Files: `scope.rs`, `binding/contribute.rs`, `binding/mod.rs`, `binding/handle.rs`,
`graph/mod.rs`, `graph/scopes.rs`, `graph/visibility.rs`, `graph/wire.rs`,
`transport/controller.rs`, `transport/enhancer.rs`, `transport/pipeline.rs`, `execution/cache.rs`,
`execution/mod.rs`, `app/shared.rs`, `resolver.rs`, `error/wiring.rs`.

## Entries

### 1. `Contribute::with` and `try_with` register `Auto`

- **Brief:** register `ScopeKind::Auto` directly; remove `BindingRecord::scope_by_role` and
  `scopes::resolve_closure_scopes`.
- **Core:** both call `push_factory::<_, Auto, _, _, _>`, the path `singleton` and its siblings
  take. The flag, its initialiser, `push_by_role` and the pass are gone, and `freeze` no longer
  calls it. Wave 5's split of `push_factory` into `factory_record` plus a push existed only for
  `push_by_role`, so it is folded back.
- **What decides the rest:** the existing `Auto` logic. A provider contribution is a singleton,
  refused by `check_scopes` when it needs an execution. A contribution under a role key is an
  enhancer from `mark_role_contributions` and is promoted to per-execution by `needs_execution`
  only when something it reads needs one.
- **Kept from wave 5:** `check_scopes` tests `effective == PerExecution` with a hook or check,
  whatever the declared scope (wave 5 entry 5). Only `Auto` bindings reach that test again, which
  the narrower test would also catch; the broader one stays because it costs nothing and covers
  any future record that is per-execution with hooks. Its doc comment drops the closure case.

### 2. `ExplicitScope` refuses `Auto` on the `_with_in` forms

- **Brief:** bound `S` so `Auto` is not accepted, or accept `Auto` as a synonym, and log it.
- **Core:** `pub trait ExplicitScope: Scope` in `ulo::scope`, implemented for `Singleton`,
  `PerExecution` and `Transient`. Its `on_unimplemented` reads "`Auto` is not an explicit scope",
  label "name `Singleton`, `PerExecution` or `Transient` here", and notes the `_with` forms.
- **Why refuse:** one spelling per meaning. A `_with_in::<Auto>` would be a second way to write
  `_with`.
- **Not re-exported at the crate root.** `Scope` and `HookCapable` are; `ExplicitScope` is
  reached as `ulo::scope::ExplicitScope`, where the markers it bounds live. M's expansion names
  the marker, not the trait.
- **Checked:** a temporary probe in `scope.rs` calling `fn explicit<S: ExplicitScope>()` with
  `Singleton` and `Auto` compiled the first and failed the second with E0277 and the message
  above. The probe was removed.

### 3. The `_with_in` methods and their generic order

- **Brief:** `guard_with_in::<S>(build)`, `interceptor_with_in::<S>(build)`,
  `error_handler_with_in::<S>(build)`; M emits these names.
- **Core:** `fn guard_with_in<S, Args, F>(&mut self, build: F) -> &mut Self` with
  `S: ExplicitScope, F: Factory<Args>, F::Output: Guard<T>`, and the same for the other two.
- **Generic order is `S, Args, F`.** Rust takes no partial turbofish on a function, so
  `guard_with_in::<S>(build)` does not compile against three type parameters. M's current
  expansion writes `spec.guard_with::<__UloA, __UloF>(build)`; the explicit form is
  `spec.guard_with_in::<S, __UloA, __UloF>(build)`, or `::<S, _, _>`.
- **Checked:** a temporary probe module compiled `guard_with(|_u: Ext<User>| ..)`,
  `guard_with_in::<Singleton, _, _>`, `guard_with_in::<Transient, _, _>`, and a generic fn
  calling `spec.guard_with_in::<PerExecution, A, F>(build)`, the shape M emits. A deliberate
  `interceptor_with_in` over a non-interceptor failed, confirming the probe was compiled. The
  probe was removed.
- **`#[track_caller]`** is now on all six closure methods, so a report can say where the closure
  was declared (entry 6). Through M's generated helper fn the location is the helper's call,
  spanned at the handler.

### 4. Where the decided scope lives: `Graph::closures`, keyed by a process-unique id

- **Brief:** decide each closure entry's effective scope at wiring, reusing the needs-execution
  knowledge the input check already walks.
- **Core:**
  - `ClosureDecl<R>` gains `id: ClosureId`, `scope: ScopeKind` (as declared) and `location`.
    `ClosureId` is a `u64` from a static `AtomicU64`, assigned when the declaration is built; a
    clone of the declaration shares it.
  - `EnhancerDep::Closure` carries a `ClosureDep`: the id, the declared scope, the dependencies,
    and the role, tier and position a report names. `EnhancerSpec::deps` takes the tier name, and
    `Mount::handler` passes `"controller"` and `"method"`.
  - `scopes::closure_scopes(graph, declared, errors)` runs in `check` after `check_scopes` and
    before `check_closures`, over the handlers this wiring added. It writes
    `Graph::closures: HashMap<ClosureId, Effective>`.
- **The rule:** `Auto` is `Singleton` (built once) unless a parameter reads execution data, then
  `PerExecution`. `PerExecution` and `Transient` are taken as written. An explicit `Singleton` that
  reads execution data is refused (entry 6).
- **Reuse:** the read test `check_closures` applied to hook and readiness closures is now
  `execution_read(graph, module, owner, dependency)`, shared by both passes: `Ext` or
  `ExecutionRef`, an execution input, or a binding for which `passes_execution` holds. The input
  walk's own logic reads inputs only, so the hook check's test is the complete one.
- **Why the graph and not the declaration:** the graph is the immutable result of wiring, the
  pipeline already holds it, and a lazy load's copy carries the map. Writing into the shared
  `ClosureDecl` during wiring would leave a value behind in a wiring that fails.
- **Lazy load:** a load mounts no handlers (`LoadRefusal::Controllers`), so `closure_scopes`
  decides nothing new in one, and the base entries arrive with the cloned graph.
- **The input walk skips a closure decided `Singleton`.** A once-built `Auto` closure reads no
  execution data, so skipping it changes nothing. An explicit singleton that reads an input is
  reported once, as entry 6's refusal, and not a second time as `InputNotSeeded`. This matches a
  singleton binding, which the walk never enters.

### 5. Pipeline: once-built, per-execution and transient closures

- **Brief:** a once-built closure enhancer is built on first use and shared across calls in a
  runtime-free once-cell; a per-execution one once per execution; a transient one at every
  obtain. Each guard is still built only after the previous one admits.
- **Core:** `obtain` hands a `Decl::Closure` to `build_closure`, which reads
  `Graph::closures`:
  - `Singleton`: `AppShared::closures`, a new store, `get_or_build(Slot::Closure(id), ..)`.
  - `PerExecution`: the execution's cache, `get_or_build(Slot::Closure(id), ..)`.
  - `Transient`, or no entry: the closure's build, as every closure was built before.
- **One cell type for both stores.** `ExecCache` became `OnceCells`, keyed by
  `Slot::{Binding(BindingId), Closure(ClosureId)}`, on `async_lock::OnceCell` as before. A
  failed build is not cached and the next call retries, as for an execution-scoped binding.
- **App-level, not in the declaration.** A cell inside `ClosureDecl` would avoid one map lookup
  and a downcast. It would also let an `EnhancerSpec` held in a static and cloned into two apps'
  mounts share one instance built from the first app's singletons. Keyed by `ClosureId` on
  `AppShared`, the instance lives and drops with the app, like a singleton.
- **A once-built closure resolves outside the call's execution**, through
  `Resolver::without_execution`, with the controller module's visibility. A read the wiring pass
  did not see then fails as `ExecutionRequired` rather than sharing one call's data with every
  later call. The `dispatch` doc states the exception.
- **No entry means transient.** `MountedHandler` is built only from graph handlers, and every
  graph handler's closures get an entry, so the arm does not run. Building at every call is the
  fallback that shares nothing.
- **Guard order is unchanged:** `admit` still obtains and checks one guard at a time.
- **Not changed:** a closure build runs under no `CONSTRUCT_TIMEOUT`, and a panic in it is caught
  by the stage's `caught` as `PanicRecovered`, as before. A once-built closure's build is that of
  the first call to reach it; a call cancelled mid-build hands the cell to the next waiter.
- **Lookup:** `Lookup` gains `exec: &ExecShared`, taken from `dispatch`'s `ExecutionRef`, so the
  per-execution arm needs no `Option`.

### 6. `ClosureScopeViolation`: an explicit singleton closure enhancer that needs an execution

- **Brief:** refuse it like any singleton needing an execution; reuse `ScopeViolation` or add a
  variant, and log it.
- **Core:** a new variant, `WiringError::ClosureScopeViolation { closure: String, role: &'static
  str, path: Vec<String>, at: &'static Location<'static> }`, in step 5.
- **Why not `ScopeViolation`:** its `binding` is a `KeyName`, and a closure enhancer on a handler
  has no key. `WiringError` is `#[non_exhaustive]`, so the variant is an addition.
- **Rendered:**

  ```text
  × scope violation: method-level guard #2 of UsersController::get (Http), declared by closure as a singleton, depends on per-execution data
    ├─ Ext<CurrentUser> (param #1)
    ├─ declared at src/users.rs:40
    └─ help: build it per call: drop the scope (`with = ..`, `guard_with`), or declare it `with(execution) = ..` (`guard_with_in::<PerExecution>`)
  ```

  `#n` counts every declaration of that role in that tier, by type and by value included, in the
  order written. The help derives the method name from `role`.

### 7. F300: `ScopeViolation` names the closure's explicit form

- **F300's first half, `HooksOnPerExecution`:** resolved by the rule change, with no code
  change. A contribution declared by closure is `Auto`, so an enhancer one is per-execution only
  when it reads something that needs an execution, and "remove what makes `X` need an execution"
  is now advice it can follow. The variant's doc adds "a contribution declared by closure
  included".
- **F300's second half, `ScopeViolation`:** a new field `by_closure: bool`, set from
  `BindingRecord::by_closure()`: `Recipe::Factory` and not `constructs`. When set:
  - the head for an `Auto` binding reads "is a provider declared by closure with no scope, so a
    singleton, and depends on per-execution data";
  - the help reads "declare the closure `with(execution) = ..`, or register it with
    `.execution(..)`", for `Auto` and explicit singletons alike.
- **Wider than `Contribute::with`.** The brief's suggested test, `Auto` plus a factory recipe,
  covers `with` only. `Recipe::Factory` without `constructs` also covers `Contribute::singleton`
  (`with(singleton) = ..`), `ModuleDef::singleton` (a providers-list closure) and their `try_`
  forms, which printed `#[injectable(execution)]` too (wave 5 entry 6). `provide_with` sets
  `constructs`, its scope coming from the type, so it keeps the attribute help.
- **One edge:** a test override of an `also_as` key splits off a record with the override's
  recipe and `constructs` false. When that recipe is a factory, the record reads as declared by
  closure, and a scope violation on it gets the closure help.
- **Breaking:** `ScopeViolation` gains a field. A downstream pattern that names every field
  without `..` stops compiling. Nothing in the crate does.
- **`HookHost`'s doc** now says an `Auto` binding includes a contribution registered through
  `Contribute::with` (wave 5's R2, and F300's candidate fix).
- **Is F300 resolved?** In the core, yes, on one condition: the help names `with(execution) = ..`
  as the macro spelling in every position, the providers list included. If M spells a
  providers-list closure's explicit scope differently, the help misdirects there. F299's closing
  note says `with(singleton | execution | transient) = ..` is built in every position.

### 8. Docs updated with the change

- `scope.rs`: the module doc states scope as its own axis and the fresh-state case; `Auto`'s doc
  drops the role clause; `HookCapable`'s drops the closure-enhancer clause.
- `Contribute`: the struct doc drops the role rule and names `with` as `Auto`; `with`'s doc states
  the `Auto` rule and the fresh-state case.
- `EnhancerSpec`: the struct doc states the rule, the `_with_in` override and the fresh-state
  case; each `_with` and `_with_in` method carries its own.
- `graph/scopes.rs`: the module doc's table row reads "Auto with hooks, inferred
  per-execution", followed by a paragraph on bindings and enhancers declared by closure;
  `closure_scopes` carries its table; `check_closures` and `check_inputs` name the closure split.
- `graph/wire.rs`: `freeze` drops `resolve_closure_scopes`; `check`'s doc places the closure pass.
- `transport/pipeline.rs`: step 2 of the module doc and `dispatch`'s doc.
- `transport/controller.rs`: `EnhancerDep::Closure` no longer says "per execution".

## Requests for other files

- **R1, M, `ulo-macros/src/enhancers/mod.rs`:** the module doc's `with = |param: Ty, ..| expr`
  bullet says "by closure, built per execution". Under this rule it is `Auto`. The explicit form
  lowers to `spec.guard_with_in::<S, __UloA, __UloF>(build)` (entry 3), `S` one of
  `ulo::scope::{Singleton, PerExecution, Transient}`.
- **R2, M, `ulo-macros/src/module_attr/providers.rs`:** the module doc's paragraph after the
  `into` table describes the role rule ("the core resolves the scope at freeze from the role. An
  enhancer is built per execution").
- **R3, design, DESIGN.md §4 and §7:** whatever still states the role rule or "built per
  execution" for a closure enhancer. The fresh-state case (a closure reading nothing per call
  that creates per-call state) needs the explicit form, which RESPONSE.md asks the docs to say.
- **R4, FRAMEWORK_GAPS.md F300:** the entry still describes the role rule. Entry 7 above is its
  status.
