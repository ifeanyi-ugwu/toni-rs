# Divergences: wave 2, agent R (keys, resolver, errors, records, pipeline, shutdown close)

Every place R's code departs from `DESIGN.md`, or fills a gap the design leaves that a user would
see. Each entry gives what the design says, what R wrote, and why. Requests for other agents'
files follow the entries.

## Entries

### 1. The text of `LookupError::Ambiguous`

- **Design:** §8.2 and §10.2 give the variant, `Ambiguous { key: KeyName, sources: Vec<ModuleName> }`,
  and no `Display` text.
- **R:** "`dyn UserRepo` is ambiguous between PersistenceModule, LegacyRepoModule", the form
  `AmbiguousModule` already writes. `key` carries `BindingKind::Single`: only a single read meets
  an ambiguous entry, since a collection read gathers from every module. `ModuleRef::get`, `get`
  on an execution opened in a non-root module and `by_key` all reach it through
  `Resolver::locate`, and `Option<S>` propagates it. `AmbiguousModule` is back to naming a module
  type alone.
- **Why:** one wording for both ambiguities, and the key prints as every other `LookupError`
  prints it.

### 2. A handler's or inner interceptor's panic passes back through the interceptors

- **Design:** §7 step 4: `dispatch` catches a panic in a guard, an interceptor, the handler or an
  error handler and offers the error handlers `PanicRecovered { stage, message }`. Silent on
  whether the interceptors around a panicking handler see it.
- **R:** the handler and each interceptor run under their own catch inside `Next::run`. A panic
  in the handler becomes the `Err(PanicRecovered { stage: Handler, .. })` that the innermost
  interceptor's `next.run()` returns, and each interceptor further out receives whatever the one
  inside it answered. An interceptor can therefore reshape or answer a handler's panic before the
  error handlers see it, as it can any handler error. `Next::run`'s doc states this.
- **Why:** a panic unwinding through the interceptors drops each of them mid-await and leaves no
  frame that knows which stage panicked. Catching at each boundary keeps `stage` exact and gives
  a timing or logging interceptor the failed call it would otherwise lose.

### 3. Which panics become `PanicRecovered`

- **Design:** §7 step 4 lists the four stages; §3.4 and §10.2 report a build inside a call that
  panics as `LookupError::Construct { reason: Panicked }`.
- **R:** a panic while the container builds an enhancer or the controller, a per-execution
  by-type guard for example, stays `LookupError::Construct { reason: Panicked(..) }`, as
  `AppShared::build` already catches it. A panic in a by-closure declaration's closure
  (`#[guards(with = |..| ..)]`) is reported as `PanicRecovered` with the stage of the enhancer it
  builds. A guard's or interceptor's synchronous code before its first await counts as that
  stage: the call happens inside the caught future.
- **Why:** a container construction has one panic report already, the one every other build
  inside a call gives. A by-closure declaration is the enhancer's own code, run by the pipeline
  rather than by the container.

### 4. `PanicRecovered` and `DispatchStage` impls and text

- **Design:** §10.2 gives the two types and their fields, and no impls.
- **R:** both implement `Debug` and `Display`; `PanicRecovered` implements `Error`.
  `DispatchStage` also derives `Clone`, `Copy`, `PartialEq`, `Eq` and `Hash`. The text is "the
  guard panicked: <message>", with `interceptor`, `handler` and `error handler` for the other
  stages. The message is `redact_panic`'s output, so the `Redacted` holds the payload text and
  scrubs registered secrets and URL userinfo from what it prints.
- **Why:** the neighbouring error types carry the same impls, and a transport rendering
  `PanicRecovered` unclaimed may log it.

### 5. A transport's `close` timeout is a `FailureReason::TimedOut` inside `source`

- **Design:** §9.5 step 6 and Bounds: a `close` that exceeds its bound is dropped and recorded as
  `ShutdownFailure::Close`. Silent on what `source` holds for a timeout.
- **R:** `source` is the redacted `FailureReason::TimedOut { after, limit }`: `limit: ShutdownCap`
  with the cap's configured duration when the cap bounded the `close`, including the first-poll
  case after expiry, and `limit: Default` with `hook_timeout` when no cap is set. `FailureReason`
  gains `impl Error`, which `Redacted` needs to hold it, and a caller tells a timeout from the
  transport's own error with `source.downcast_ref::<FailureReason>()`. The text reads "transport
  `Http` failed to close: timed out after 5s (`shutdown_timeout`)". `Limit`'s docs now name the
  close beside the hook.
- **Why:** §10.2 already has a type that names which limit fired and its configured duration, and
  a new public error type for one case would duplicate it. `NoTimer` sets the precedent of a typed
  value inside a `Redacted` that a caller downcasts.

### 6. How the close bound is taken

- **Design:** §9.5 step 6: each `close` runs in reverse bind order, one after another, bounded by
  what is left of `shutdown_timeout`, or by `hook_timeout` when no cap is set.
- **R:** the bound is read as each `close` starts, so the cap's remainder shrinks with every close
  before it. The `close` is polled before its bound, so one that completes on the poll where the
  bound fires counts as closed, the tie rule `lifecycle::run` uses for hooks. Without a `Timer`
  there is neither bound and `close` runs unbounded, by §3.9's rule that app defaults apply only
  with a `Timer`. `listen()` refuses a transport on an app with no `Timer`, so a timerless app has
  no server to close.
- **Why:** the reading the design's wording gives; the timerless case follows §3.9 and cannot
  occur for a bound server.

### 7. `{:#}` on `Key` and `KeyName`, and their `Debug`

- **Design:** §10.1: a report that would print two different keys alike prints them with full
  paths. Silent on the spelling.
- **R:** `format!("{key:#}")` writes `type_name`'s full path on both sides of `@`, as in
  `my_app::db::PgPool @ my_app::Replica`, and `KeyName` adds ` (collection)` as before. `{}` is
  unchanged. `Debug` for both delegates to `Display` and passes the flag along, so `{:#?}` also
  prints full paths.
- **Why:** the alternate flag is the formatting-time switch §10.1 asks for, and `Debug` stays one
  text with `Display`.

### 8. `Requirement` and `Dependencies` are `Clone`

- **Design:** D20 signs `Clone` on the internal records; `Requirement` and `Dependencies` are
  public (§3.2).
- **R:** both derive `Clone`, beside the internal records `BindingRecord`, `AlsoAs`, `Recipe`,
  `ReadyRecord`, `HookRecord`, `HandlerDecl`, `EnhancerDep`, `HandlerRecord`, `ControllerRecord`,
  `DependencyRecord`, `Read` and `ReadKind` (the last three, `ControllerRecord` and the existing
  `Qualifier` and `DependencyLabel` also `Copy`). Every closure field was already an `Arc`, so no
  constructor changed, and a clone shares each closure and value with the original.
- **Why:** a `BindingRecord` holds a `Dependencies`, which holds `Requirement`s; deriving on the
  record needs them `Clone`. The impl is additive on the public types.

## Requests for other agents

### R1. W, `crates/ulo/src/transport/mod.rs`: `pub trait Role`

- **Item:** `Role`, sealed, implemented for `AnyGuard<T>`, `AnyInterceptor<T>` and
  `AnyErrorHandler<T>` (§3.7).
- **Request:** define it at that path. `lib.rs` now carries `pub use transport::Role;`, which
  fails to resolve until the trait exists.

### R2. W, `crates/ulo/src/graph/wire.rs`: replace the hand copies with `.clone()`

- **Item:** `clone_record`, `clone_ready`, `clone_hook`, `clone_recipe`, `clone_handler`,
  `clone_dependencies`, `clone_read`.
- **Request:** every record in R's files derives `Clone` (entry 8), so each of these is
  `.clone()` on its argument. With W's own graph records derived too, `clone_graph` reduces to
  `base.clone()`, which is D20's signed outcome.

### R3. W, `crates/ulo/src/error/wiring.rs`: the full-path form

- **Item:** the report's check for two keys that print alike (§10.1).
- **Request (coordination):** `{:#}` on a `KeyName` or `Key` writes both sides of `@` in full and
  keeps the ` (collection)` suffix (entry 7). `short_type_name` is unchanged for the short form.

### R4. W, `crates/ulo/src/transport/mod.rs`: the `ErrorHandler` doc

- **Item:** the doc comment on `ErrorHandler`, which reads "A guard's refusal arrives as
  `GuardRejected`".
- **Request:** add that a panic in a guard, interceptor, handler or earlier error handler arrives
  as `PanicRecovered` (entries 2 and 3).
