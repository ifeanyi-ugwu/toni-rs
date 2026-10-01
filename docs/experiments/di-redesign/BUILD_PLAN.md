# Build plan: filling in the `ulo` and `ulo-macros` spine

The spine is the module tree, every public signature and the `pub(crate)` structures that cross
module boundaries, with `todo!()` bodies. Seven areas fill it in parallel. Each area owns its
files and builds against the signatures the other areas' files already carry.

`DESIGN.md` is the spec. `RESPONSE.md` breaks a tie inside it, the latest response winning.
`divergences/spine.md` lists every place the spine already departs from the design or fills in
what it leaves unspecified; those entries await the user's sign-off like any area's.

## Rules

1. **No `cargo` in any form.** No `check`, `build`, `test`, `fmt`, `clippy`, `expand`. The user
   compiles once the whole race is done.
2. **No tests and no examples.**
3. **Own files only.** An area edits the files assigned to it below and no others. A change needed
   in another area's file, including a new `pub(crate)` item or a changed signature, is written as
   a request in `divergences/<area>.md` naming the file, the item and the reason; the coordinator
   routes it to the owner.
4. **The contracts below are frozen.** A `pub(crate)` item listed under "Cross-area contracts"
   keeps its name, fields and signature. Its owner may add private helpers and fields no other area
   reads; changing what another area calls goes through rule 3.
5. **Public signatures are frozen.** A public item keeps the signature the spine gives it. Adding,
   renaming or reshaping a public item is a divergence.
6. **Every divergence is logged** in `divergences/<area>.md`, one entry each: what `DESIGN.md` says
   (or that it is silent), what was written, and why. A behaviour the design states and the code
   does differently is a divergence even when no signature changes. Internal-only choices need no
   entry unless they shape the public API.
7. **No git commands that commit, switch branches or stash.** The coordinator commits.
8. **Code style.** Rust 2024, stable, MSRV 1.88. The core depends on no async runtime: `std::future`,
   `BoxFuture`, the `Timer` trait and `async-lock` only. Comments say what the code cannot: why,
   ordering constraints, invariants; public docs say when to use an item and what the design states
   about it. No labels, no step narration, no section dividers.
9. **Nothing panics or exits** on a path the design gives a typed error (§10.2 [45]). `todo!()` is
   the only panic the spine leaves, and every one is replaced.

## Areas and files

Every source file of both crates belongs to exactly one area.

### A. Keys, sites and the resolver

| File | Holds |
|---|---|
| `crates/ulo/Cargo.toml` | the core's manifest |
| `crates/ulo/src/lib.rs` | the public re-exports; other areas request additions through rule 3 |
| `crates/ulo/src/key.rs` | `Key`, `KeyName`, `BindingKind`, `short_type_name` |
| `crates/ulo/src/scope.rs` | scope markers, `Scope`, `ScopeKind`, `HookCapable`, `AllowedIn` |
| `crates/ulo/src/site/mod.rs` | `Site`, `SiteDesc`, `Sites`, `SiteRead`, `ReadKind`, `SiteRecord`, `SiteLabel` |
| `crates/ulo/src/site/dep.rs` | `Dep` and its `Site` impl |
| `crates/ulo/src/site/many.rs` | `Many` and its `Site` impl |
| `crates/ulo/src/site/ext.rs` | `Ext` and its `Site` impl |
| `crates/ulo/src/site/option.rs` | `Site for Option<S>` |
| `crates/ulo/src/site/handles.rs` | `Site` for `ModuleRef` and `ExecutionRef` |
| `crates/ulo/src/resolver.rs` | `Resolver`, `Purpose`, `Entries`, `Entry` |

Design sections: §0.5, §3.1, §3.2, §3.3, §5, §8.2 (a lookup naming no module uses the root's
visibility), §10.2 (`LookupError`: `NotFound`, `WrongType`, `WrongKind`, `ExecutionRequired`),
§13 (Sites row), §14.1, §14.2, §14.10.

### B. Bindings, the value API's records, handles, `Timer` and `Bound`

| File | Holds |
|---|---|
| `crates/ulo/src/timer.rs` | `Timer`, `Bound`, `BoxFuture`, `BoxError`, `BoundKind`, `Resolved`, `Defaults`, `resolve_bound`, `timeout` |
| `crates/ulo/src/construct.rs` | `Construct`, `ConstructError` |
| `crates/ulo/src/hooks.rs` | the five lifecycle traits, `HookKind`, `Hooks<T>`, `TraitHook`, `HookRecord`, `HookFn`, `HookCx`, `erase_trait_hooks`, `hooks!` |
| `crates/ulo/src/binding/mod.rs` | `Instance`, `ErasedCtor`, `Coercion`, `CheckFn`, `BindingRecord`, `Qualifier`, `AlsoAs`, `Recipe`, `ReadyRecord`, `erase_construct`, `coercion` |
| `crates/ulo/src/binding/factory.rs` | `Factory`, `ShutdownFactory`, the arity impls, the `erase_*` helpers |
| `crates/ulo/src/binding/handle.rs` | `Handle` and its typestate |
| `crates/ulo/src/binding/contribute.rs` | `Contribute` |
| `crates/ulo/src/binding/alias.rs` | `Alias`, `Input` |

Design sections: §3.4, §3.5, §3.9, §4, §6.1, §9.1, §9.3 (the readiness builder and its bounds),
§12 (the typestate rows), §13 (Bindings, Binding handles and Construction rows), §14.14.
Probes `p01`, `p04`, `p05`, `p12`, `p16`, `p19` hold working versions of the coercion closure,
the hook probes, the erased constructor slot, the arity factory and the bounded typestate.

### C. Modules, the graph and the wiring pass

| File | Holds |
|---|---|
| `crates/ulo/src/module/mod.rs` | `Module`, `ModuleIdentity`, `ConfigKey`, `DynKey`, `ModuleName` |
| `crates/ulo/src/module/def.rs` | `ModuleDef`, `ModuleHook`, `ModuleNode` and its records |
| `crates/ulo/src/module/dynamic.rs` | `DynamicModule` |
| `crates/ulo/src/module/keyed.rs` | `Keyed` |
| `crates/ulo/src/module/meta.rs` | `Meta`, `MetaMap`, `MetaEntry`, `FrozenMeta` |
| `crates/ulo/src/graph/mod.rs` | `Graph`, `ModuleId`, `BindingId`, `FrozenModule`, `FrozenBinding`, `Role`, `Effective`, `Edge`, `EdgeTarget`, `VisibilityTable`, `Visible`, `InputDecl` |
| `crates/ulo/src/graph/register.rs` | `Registry`, `Import`, `register_all`, `register_lazy` |
| `crates/ulo/src/graph/wire.rs` | `WireEnv`, `wire`, `wire_lazy`, `LazyWiring`, `LazyFailure`, `freeze`, steps 1, 2 and 6 |
| `crates/ulo/src/graph/visibility.rs` | step 3 |
| `crates/ulo/src/graph/cycles.rs` | import cycles, step 4 |
| `crates/ulo/src/graph/scopes.rs` | step 5: roles, needs-execution, scope violations, the input check |
| `crates/ulo/src/graph/order.rs` | collection order, connect order |

Design sections: §3.6, §6.2, §6.4, §7 (role assignment; a by-type enhancer resolved against the
controller module's visibility), §8.1–§8.4, §9.2 (the orders), §10.1, §11 (override and
replacement rules as wiring applies them), §12 (startup rows), §14.3, §14.4, §14.7, §14.8.

### D. Executions, the `App` typestates, `AppHandle`, `execute`, `load`, `TestApp`

| File | Holds |
|---|---|
| `crates/ulo/src/execution/mod.rs` | `Execution`, `ExecutionRef`, `ExecOptions`, `ExecShared` |
| `crates/ulo/src/execution/cache.rs` | `ExecCache` |
| `crates/ulo/src/execution/extensions.rs` | `Extensions`, `Inputs` |
| `crates/ulo/src/execution/notify.rs` | `Notify`, `Listen`, `Cancelled`, `Draining` |
| `crates/ulo/src/app/mod.rs` | `App`, `Wired`, `Connected`, `Bound`, `AppBuilder`, `AppConfig` |
| `crates/ulo/src/app/handle.rs` | `AppHandle` |
| `crates/ulo/src/app/shared.rs` | `AppShared`, `SingletonStore`, `LiveSet`, `LiveSlot` |
| `crates/ulo/src/app/load.rs` | `load` |
| `crates/ulo/src/module/handle.rs` | `ModuleRef` |
| `crates/ulo/src/testing.rs` | `TestApp`, `Settled`, `Pending`, `TestPlan` and its records |

Design sections: §2, §3.8, §6.3, §8.5, §8.6, §9.4, §11, §14.6, §14.9.
Probes `p10`, `p14` (execute's `AsyncFnOnce` and `Send`) and `p18` (the token and the phase check).

### E. Lifecycle, connect, shutdown, redaction and errors

| File | Holds |
|---|---|
| `crates/ulo/src/lifecycle/mod.rs` | module declarations |
| `crates/ulo/src/lifecycle/phase.rs` | `Phase`, `PhaseCell` |
| `crates/ulo/src/lifecycle/run.rs` | `Cap`, `Outcome`, `run`, `CatchUnwind`, `select`, `Either` |
| `crates/ulo/src/lifecycle/connect.rs` | `connect`, `build_singleton`, `run_readiness`, `run_startup_hooks` |
| `crates/ulo/src/lifecycle/shutdown.rs` | `ShutdownCell`, `close`, `run_sequence`, `run_hook_step`, `drain` |
| `crates/ulo/src/signal.rs` | `Signal` |
| `crates/ulo/src/redact.rs` | `Redacted`, `Secret`, `SecretRegistry`, `redact`, `redact_panic`, `strip_userinfo` |
| `crates/ulo/src/error/mod.rs` | every §10.2 type but `ConstructError`, plus `LookupKind` |
| `crates/ulo/src/error/wiring.rs` | `WiringErrors`, `WiringError` and the report format |

Design sections: §3.9 (the bound rule and the drain asymmetry), §9.2, §9.3 (readiness run
semantics and redaction), §9.5, §10.2, §12 (connect, listen and close rows), §14.5, §14.11–§14.17.
Probe `p20` holds a working `Redacted` and `strip_userinfo`.

### F. The transport SPI and the enhancer pipeline

| File | Holds |
|---|---|
| `crates/ulo/src/transport/mod.rs` | `Transport`, `Guard`, `Interceptor`, `ErrorHandler`, the erased twins, `AnyGuard`, `AnyInterceptor`, `AnyErrorHandler` |
| `crates/ulo/src/transport/next.rs` | `Next` |
| `crates/ulo/src/transport/enhancer.rs` | `EnhancerSpec`, `Decl`, `ClosureDecl` |
| `crates/ulo/src/transport/controller.rs` | `Controller`, `Mount`, `MountedHandler`, `ControllerRecord`, `HandlerDecl`, `EnhancerDep`, `HandlerRecord` |
| `crates/ulo/src/transport/server.rs` | `Server`, `Mounted`, `DrainToken`, `ErasedServer` |
| `crates/ulo/src/transport/pipeline.rs` | `dispatch` |

Design sections: §3.7, §6.3 (`open_terminal` and `DrainToken` from the transport's side), §7,
§9.5 step 2, §13 (Transports row). Probe `p01` holds the erased role twin.

### G. Macros

| File | Holds |
|---|---|
| `crates/ulo-macros/Cargo.toml` | the macro crate's manifest |
| `crates/ulo-macros/src/lib.rs` | every entry point |
| `crates/ulo-macros/src/shared/mod.rs` | `ulo()`, `ConstructImpl` |
| `crates/ulo-macros/src/shared/attrs.rs` | attribute matching |
| `crates/ulo-macros/src/shared/sites.rs` | `SiteSpec`, `SiteLabel`, `declare`, `read` |
| `crates/ulo-macros/src/injectable/mod.rs` | `#[injectable]` dispatch |
| `crates/ulo-macros/src/injectable/args.rs` | `InjectableArgs`, `ScopeArg` |
| `crates/ulo-macros/src/injectable/struct_form.rs` | the struct form |
| `crates/ulo-macros/src/injectable/impl_form.rs` | the constructor form |
| `crates/ulo-macros/src/construct_attr.rs` | `#[construct]` out of place |
| `crates/ulo-macros/src/module_attr/mod.rs` | `#[module]` |
| `crates/ulo-macros/src/module_attr/args.rs` | `ModuleArgs`, `ExportEntry` |
| `crates/ulo-macros/src/module_attr/providers.rs` | `ProviderEntry` and its lowering |
| `crates/ulo-macros/src/routes/mod.rs` | `#[routes]` |
| `crates/ulo-macros/src/enhancers/mod.rs` | the enhancer attributes, `__handler`, `__enhancer_specs!` |
| `crates/ulo/src/__private.rs` | what generated code calls: `assert_site`, the role assertions, `IntoConstructed`, the hook and factory probes |

Design sections: §0.2, §4 (the `#[module]` lowering), §5 (compile-time site checks), §6.1, §7 (the
attributes and the strict controller-level rule), §8.1, §9.1 (probing), §10.3, §12 (compile rows).
Probes `p02c` (autoref over a closure's output) and `p04` (hook probes).

## Cross-area contracts

Each row is an item one area calls in another. The owner keeps it as the spine writes it.

### Owned by A

| Item | Called by | For |
|---|---|---|
| `Resolver::new(app, module, exec, purpose)`, `Resolver::in_module`, `Purpose` | D, E | building a resolver for a lookup, a construction or a lifecycle hook |
| `Resolver::instance(id)` | A's own site reads; F through `Entry::resolve` | one binding's instance, delegated to `AppShared::obtain` |
| `Resolver::entries`, `Entry::resolve` | F | the lazy guard walk |
| `SiteRead`, `ReadKind`, `SiteRecord`, `SiteLabel`, `Sites.list`, `SiteDesc.reads` | C | resolving sites into edges and naming them in reports |
| `Key::{requalified, with_qualifier, type_id, qualifier_id, type_name, qualifier_name, is_unqualified, name}` | B, C, E | requalification and diagnostics |
| `short_type_name` | C, E | every diagnostic type name |

### Owned by B

| Item | Called by | For |
|---|---|---|
| `Instance`, `instance_of`, `downcast_instance` | A, C, D, E, F | the stored form of every built value |
| `BindingRecord` and `BindingRecord::{new, keys}`, `Qualifier`, `AlsoAs`, `Recipe`, `ReadyRecord` | C (writes through `ModuleDef`, freezes), D (`obtain` dispatches on `Recipe`), E (runs `ready`, `hooks`, `construct_bound`) | the binding record |
| `ErasedCtor`, `CheckFn`, `Coercion`, `coercion`, `erase_construct` | C, D, E, F | constructing, checking and widening |
| `erase_factory`, `erase_try_factory`, `erase_check`, `erase_init_hook`, `erase_destroy_hook`, `erase_signalled_hook` | C (`ModuleDef` methods), D (`TestApp` overrides), F (`EnhancerSpec::*_with`) | erasing closures |
| `HookRecord`, `HookFn`, `HookCx`, `erase_trait_hooks`, `Hooks::new` | C (records module hooks), E (runs every hook) | the hook record |
| `Handle::new`, `Contribute::new`, `Alias::new`, `Input::new` | C (`ModuleDef`) | starting a handle on a pushed record |
| `BoundKind`, `Resolved`, `Defaults`, `resolve_bound`, `timeout` | D (`obtain` inside a call), E (every timed item) | the §3.9 rule |

### Owned by C

| Item | Called by | For |
|---|---|---|
| `Graph` and `Graph::{binding, module, lookup, collection, find_module}`, its fields `connect_order`, `root`, `handlers`, `secrets`, `inputs` | A, D, E, F | every runtime read of the frozen graph |
| `ModuleId`, `BindingId`, `FrozenModule`, `FrozenBinding`, `Role`, `Effective`, `Edge`, `EdgeTarget`, `Visible`, `InputDecl` | A, D, E, F | the frozen records |
| `wire::{wire, wire_lazy, WireEnv, LazyWiring, LazyFailure}` | D (`AppBuilder::wire`, `TestApp`, `load`) | the wiring pass |
| `ModuleNode` and its records (`ImportRecord`, `ExportRecord`, `InputRecord`, `ValueFailure`), `ModuleDef::new`, `ModuleDef::keyed_by` | B (handles and builders push into it) | registration |
| `ModuleIdentity::{of_owner, keyed_by, type_id, type_name, qualifier, label_text, is_configured}`, `ModuleName::of` | D, E | identity lookups and names |
| `MetaMap`, `FrozenMeta` | D (`ModuleRef::meta`), F (`MountedHandler`) | module metadata |

### Owned by D

| Item | Called by | For |
|---|---|---|
| `AppShared` and its fields; `AppShared::{new, graph, root, module_ref, obtain, open, execute}` | A, E, F | the shared state, instance lookup, opening executions |
| `SingletonStore::{get, insert}` | E (connect stores, lookups read) | the singleton store |
| `LiveSet::{enter, attach, until_empty, cancel_all}`, `LiveSlot` | E (the drain) | live executions |
| `AppConfig::{defaults, drain, knobs_set}` and its consts | C (`WireEnv`), E | timing configuration |
| `ExecShared` and its fields, `ExecShared::{resolver, cancelled, draining, is_draining}` | A, E, F | per-execution state |
| `ExecCache::get_or_build`, `Inputs::{insert, get, get_erased}` | A (site reads through `obtain`) | the cache and inputs |
| `Notify::{new, fire, is_fired, listen}`, `Listen`, `Cancelled::new`, `Draining::new` | E | the waker list |
| `ModuleRef::new`, its fields | A, F | module handles |
| `App::from_shared` | E | state transitions after connect and listen |
| `TestPlan`, `Override`, `OverrideTarget`, `CollectionOverride`, `Replacement` | C | applying overrides and replacements at wiring |

### Owned by E

| Item | Called by | For |
|---|---|---|
| `Phase`, `PhaseCell::{new, get, advance, allows_execution, allows_singleton_lookup, allows_load}` | D | phase checks on open, lookup and load |
| `lifecycle::run::{run, Cap, Outcome, CatchUnwind, select, Either}` | D (a build inside a call) | the bounded, panic-catching runner |
| `lifecycle::connect::connect` | D (`App::connect`, `load`) | the connect walk |
| `lifecycle::shutdown::{ShutdownCell, close, run_sequence}` | D (`serve`, `close`) | the one shutdown |
| `redact`, `redact_panic`, `SecretRegistry::{register, register_if_secret}`, `Redacted::from_parts` | B, C, D, F | redaction at every store of an outside error |
| `WiringErrors::new`, `WiringError` variants | C | the wiring report |
| `Closed::new`, `GuardRejected::new`, every error variant | A, C, D, F | constructing errors |

### Owned by F

| Item | Called by | For |
|---|---|---|
| `ControllerRecord`, `Mount { handlers }`, `HandlerDecl`, `EnhancerDep`, `HandlerRecord` | C (`ModuleDef::controller`, wiring calls `mount`, freezes handlers) | handler registration |
| `HandlerRecord::mounted` | F's own `ErasedServer::bind` | typed handlers at bind |
| `ErasedServer` and its blanket impl | D (`listen` binds, stores), E (drain, close) | the stored server |
| `DrainToken::new` | E (stop accepting) | the drain token |
| `Decl`, `ClosureDecl` | C (step 3 resolves enhancer declarations) | enhancer dependencies |

### Owned by G

| Item | Called by | For |
|---|---|---|
| `__handler`, `__enhancer_specs!` | transport crates (a later race) | the handler protocol described in `crates/ulo-macros/src/routes/mod.rs` |
| `crates/ulo/src/__private.rs` | code `ulo-macros` generates | assertions, `IntoConstructed`, probes |

## Points each area resolves in its own log

These are not settled by the design or by the spine. The owning area decides and logs the
decision in `divergences/<area>.md`.

- **A.** How `KeyName` shortens type names (`short_type_name`), and `Key`'s `Display` text.
- **B.** Whether `Factory::call` reads sites concurrently or in order (order is the expectation:
  a cancellation at one site should not leave half-read others running).
- **C.** The exact text of every `WiringError` hint, beyond the samples in §10.1; which keys count
  as role keys for a transport with no handlers.
- **D.** How `execute` enforces a standalone deadline without a runtime (racing the closure's
  future against `Timer::sleep` while still polling it).
- **E.** Shutdown hook order across lazily loaded modules and the base graph.
- **F.** Whether `dispatch` resolves the controller per call through `exec.get::<C>()` inside the
  handler closure, or takes it from the `MountedHandler`.
- **G.** The transport scope keys (`http`, `rpc`, `grpc`, `ws`) are declared by transport crates;
  until they exist, `__enhancer_specs!` receives the key as a string literal.
