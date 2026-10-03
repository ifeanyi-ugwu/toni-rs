# Divergences: wave 3, agent R (D28, `is_panic`, D26's record half, D27's doc)

Every place R's code departs from `DESIGN.md`, or fills a gap the design leaves that a user would
see. Each entry gives what the design says, what R wrote, and why. Requests for other agents'
files follow the entries.

Nothing in the checkout has compiled. Entries 1, 3 and 5 rest on two stub probes compiled and run
with plain `rustc +1.88 --edition 2024` in the scratchpad: the `is_panic` block and
`bounded_close` extracted verbatim from the edited files, against stand-ins for the core's types.

## Entries

### 1. `ShutdownFailure::Close` holds `reason: FailureReason`, and a panicking `close` is caught

- **Design:** D28 (b): `Close { transport, reason: FailureReason }`, the transport's error as
  `Errored(Redacted)`. §9.5 and §10.2 are silent on a `close` that panics.
- **R:** as signed. A `close` exceeding its bound is `TimedOut { after, limit }`: `limit:
  ShutdownCap` with the cap's duration when the cap bounded it, the first-poll case after expiry
  included, and `limit: Default` with `hook_timeout` otherwise. A transport's `Err` is `Errored`,
  redacted. A panic is `Panicked`, redacted by `redact_panic`. `close` is called inside the caught
  future, so a hand-written `Server::close` that panics before returning its future is caught as
  well. `Skipped` never occurs on `Close`: every `close` starts. The text is unchanged:
  "transport `Http` failed to close: timed out after 5s (`shutdown_timeout`)", and "transport
  `Http` failed to close: panicked: <message>" for a panic.
- **Why:** before this, a panic in `close` was caught nowhere and unwound out of the shutdown
  sequence into whichever `serve` or `close` call was running it. `Panicked` is the outcome
  `Hook` already reports for a hook's panic.

### 2. `FailureReason` no longer implements `Error`

- **Design:** §10.2 declares `FailureReason` with no `Error` impl. Wave 2 added one [R 5] so a
  `Redacted` could hold a `TimedOut` in `Close`'s `source`.
- **R:** removed. `Display` and `Debug` stay.
- **Why:** with `Close` holding the reason as a field, nothing boxes a `FailureReason`, and no code
  under `crates/` uses the impl. Restoring it is one line.

### 3. `is_panic` takes `&BoxError`

- **Design:** the fourteenth response: "a small public helper, such as
  `ulo::is_panic(&BoxError) -> bool`". The brief offered `&(dyn Error + 'static)` as the
  alternative.
- **R:** `pub fn is_panic(error: &BoxError) -> bool`, re-exported at the crate root.
- **Why:** an error handler receives `err: BoxError`, and an interceptor gets one from
  `next.run()`. `is_panic(&err)` compiles against `&BoxError`. Against a `dyn` parameter,
  `&(dyn Error + 'static)` or `&(dyn Error + Send + Sync + 'static)` alike, it fails with E0277:
  coercion commits to unsizing the `Box` itself, and `Box<dyn Error>` does not implement `Error`.
  The caller would have to write `&*err`. Probed on 1.88: both `dyn` forms refused `&err`, and
  `&BoxError` accepted `&err` and a `&BoxError` binding. The cost: a caller holding a bare
  `&dyn Error`, such as a `source()` link, cannot pass it. `is_panic` walks `source()` itself.

### 4. `is_panic` also answers for a `ConnectError`, bare or inside `LoadError` or `StartupError`

- **Design:** the fourteenth response names two shapes of a panic during a call: `PanicRecovered`
  and `LookupError::Construct { reason: Panicked }`.
- **R:** both, and any `ConnectError` whose reason is `Panicked`, bare or as `LoadError::Connect`
  or `StartupError::Connect`.
- **Why:** a service holding a clone of `AppHandle` can call `load` inside a call (§8.6). A handler
  that propagates the `LoadError` hands the error handlers a lazily loaded module's constructor,
  readiness or init-hook panic as `LoadError::Connect(ConnectError::{Construct, Readiness,
  Hook} { reason: Panicked })`, which "any panic → 500 and an alert" would otherwise miss.
  `StartupError` never reaches an error handler; it costs one arm and keeps the function's
  statement true for every core error that carries a caught panic.

### 5. How far `is_panic` looks

- **Design:** silent beyond the two shapes.
- **R:** a dependency's `LookupError` passes up unchanged through `ConstructError::Dependency`, so a
  panic any depth down a dependency chain arrives as the top-level `LookupError::Construct {
  reason: Panicked }` naming the deepest key, with no nesting. The nesting that does occur is a
  constructor or a `try_` factory returning a dependency's `LookupError` as its own `Err`:
  `Construct { reason: Errored(Redacted(LookupError::Construct { reason: Panicked })) }`.
  `is_panic` follows `Errored`'s `Redacted`, `ConstructError::Failed` and every error's `source()`
  chain, to 32 levels. The probe asserted that case, a panic behind a user wrapper's `source()`,
  a `Redacted` inside a `Redacted`, and the negatives (`NotReady`, `Skipped`, `LoadError::Closed`,
  a plain string error).
- **Limit:** `Redacted` hands out its original by type alone (§10.2), so inside one only the
  core's types are recognized: `PanicRecovered`, `LookupError`, `ConstructError`, `ConnectError`,
  `LoadError`, `StartupError` and `Redacted`. A user error type wrapping a panicked `LookupError`,
  returned as a constructor's `Err` and redacted, is not looked through. Reaching it would need a
  `&dyn Error` accessor on `Redacted`, a third path to the original, which §10.2 rules out.

### 6. `.backoff(..)` records its location, and the last call wins

- **Design:** D26: `#[track_caller]` on `backoff`, a `backoff_location` on `ReadyRecord`.
- **R:** `backoff_location: Option<&'static Location<'static>>`, the type of every other record's
  location field, `None` until a `.backoff(..)` writes it. Each call overwrites the duration and
  the location together, matching `.backoff`'s last-write-wins. A replacing `.ready(..)` starts a
  new record with `None`. `ReadyRecord` still derives `Clone`. The doc on `backoff` now states
  that a nonzero wait on an app with no `Timer` is a wiring error naming the call.
- **Why:** the line `BackoffWithoutTimer` names is the call that set the duration it reports. A
  `.backoff(5s)` followed by `.backoff(Duration::ZERO)` reports nothing, since `wire()` tests the
  final duration.

### 7. Internal: where D27's sentence sits

`Next`'s struct doc carried "a panic further down the chain comes back from `run` as an `Err`
holding `PanicRecovered`". That sentence moved to `run`'s doc and gained D27's warning, so the
behaviour is stated once, on the method that returns the `Err`.

## Requests for other agents

### R1. The design agent, `DESIGN.md` §10.2

- **Item:** the `ShutdownFailure` declaration, the redaction paragraph, and the error API list.
- **Request:** `Close { transport: &'static str, reason: FailureReason }` replaces `source:
  Redacted`, the comment naming `Errored` for the transport's error, `Panicked` for a panic and
  `TimedOut` for the bound (entry 1). The redaction paragraph's list of `Redacted` fields drops
  "the `source` of `ShutdownFailure::Close`": that error is now stored in `FailureReason::Errored`,
  already in the list. `pub fn is_panic(error: &BoxError) -> bool` joins the section beside
  `PanicRecovered`, with entries 4 and 5's scope. `FailureReason`'s doc names a transport's
  `close` among the places it reports.

### R2. W, or this wave's owner of `crates/ulo/src/transport/`: two doc lines

- **Item:** `Server::close`'s doc in `transport/server.rs`, and the closing paragraph of
  `transport/pipeline.rs`'s module doc.
- **Request (coordination, no build impact):** `Server::close` says an error is recorded as
  `ShutdownFailure::Close`; add that a panic is recorded there too, as `Panicked`. The pipeline
  paragraph on a construction panic arriving as `LookupError::Construct` could point to
  `ulo::is_panic`, which answers true for both shapes.

### R3. W and M: `FailureReason` is not an `Error` any more

- **Item:** entry 2.
- **Request (coordination):** no code under `crates/` relied on the impl at the time of writing.
  New code that boxes a reason, `BoxError::from(reason)` or `?` into a `BoxError`, will not
  compile; hold it as a field, the way `Hook`, `Construct` and `Close` do.

### Note to W: `graph/wire.rs` needs no change

`ready.backoff_location.unwrap_or(ready.location)` in the `BackoffWithoutTimer` check matches the
field as added (entry 6).
