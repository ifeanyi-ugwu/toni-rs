# Divergences: area B (bindings, records, handles, `Timer`, `Bound`, `Construct`, hooks)

Each entry gives what `DESIGN.md` says, or that it is silent, what area B wrote, and why. All
await the user's sign-off. Requests to other areas follow the entries.

## The point BUILD_PLAN leaves to B

### 1. A factory reads its parameters in order

- **Design:** §5 says closure parameters are sites; it does not say whether a factory reads them
  together or one at a time.
- **B:** `Factory::call` and `ShutdownFactory::call` read each parameter through `Site::read`
  one after another, in the order written. The first failed read ends the call, and no later read
  starts.
- **Why:** a cancellation or failure at one site leaves no half-read others running, and the order
  of side effects in nested constructions is the order the closure lists its parameters.

## Gaps filled that a user sees

### 2. Readiness defaults: zero retries, zero backoff, `retries` counts attempts after the first

- **Design:** §9.3 writes `.retries(5)` and `.backoff(Duration::from_millis(500))`; it gives no
  default for either and does not say whether `retries(5)` means five attempts or five after the
  first.
- **B:** a check with neither written makes one attempt with no wait. `retries(n)` allows `n`
  attempts after the first, `n + 1` in all. Both are documented on the handle methods and on
  `ReadyRecord`.
- **Why:** "retries" names repeats of a first attempt. A check that writes no retry policy is
  checked once.

### 3. A failed site read in a hook closure reports as `Errored`, shutdown hooks included

- **Design:** §10.2 says `ShutdownFailure::Hook` is "never `Errored`: the shutdown hook traits
  return `()`". It is silent on how a closure hook (spine entry 7) whose site read fails is
  reported, at connect or at shutdown.
- **B:** the erased closure hook answers `Err(BoxError)` holding the `LookupError`, which the
  runner stores as `FailureReason::Errored(Redacted)`, downcastable to `LookupError`. That is
  `ConnectError::Hook { reason: Errored }` for an `on_init` closure and
  `ShutdownFailure::Hook { reason: Errored }` for an `on_destroy`, `before_shutdown` or
  `on_shutdown` closure. A trait hook never fails this way: it reads no sites.
- **Why:** the hook did not run, and a failure that never reaches the report is worse than an
  `Errored` on a hook kind whose body cannot fail. `FailureReason` has no variant for "a
  dependency of this hook failed", and `HookFn` returns `BoxError`, so `Errored` is the carrier
  available. The alternative is a new `FailureReason` variant, which is area E's type.

### 4. Hooks on one binding: trait hooks first, then closure hooks in the order written

- **Design:** §9.2 orders hooks across bindings; it is silent on the order of several hooks of one
  kind on one binding, such as an `OnModuleInit` impl beside an `.on_init(..)` closure on
  `provide_with`.
- **B:** `BindingRecord::hooks` holds `T::hooks` first, then each closure hook as its handle
  method is called. Area E runs them in that order at startup; how it reverses them at shutdown is
  E's to state.
- **Why:** the trait hooks are registered when the binding is, before any handle method runs.

### 5. A second `.qualified::<Q>()` replaces the first

- **Design:** silent. The handle's typestate (spine entry 2) keeps `qualified` available after it
  is called.
- **B:** the last call wins.
- **Why:** refusing it would cost a state parameter for a call that is redundant rather than
  conflicting, the reasoning §9.3 applies to `.retries`.

### 6. An alias record is transient

- **Design:** §4 and §11 describe what an alias reads; no section gives an alias a scope.
- **B:** `Alias::of` pushes a record with `ScopeKind::Transient`, no sites, and
  `Recipe::Alias { target: Key::of::<T, Existing>() }`, qualified with `Q`.
- **Why:** an alias holds no instance of its own, and its target's scope decides what is shared.
  Transient is the one scope that is never built at `connect` and that passes a target's need for
  an execution to the alias's readers without being refused (§6.2). A path printed through an
  alias names it `(transient)`.

### 7. `ConstructError`'s `Debug` and `source`

- **Design:** §10.2 gives the variants; spine entry 44 gives `Display` and `Error`, with `Display`
  writing the wrapped error's text.
- **B:** `Debug` writes `Site(..)` or `Failed(..)` around the wrapped error's own `Debug`;
  `source()` returns the wrapped error's `source()`, not the wrapped error.
- **Why:** with `Display` transparent, returning the wrapped error from `source()` would print its
  text twice to a reporter walking the chain. `ConstructError` is not a stored core error, and
  `Failed` holds the original by design (§10.2), so its `Debug` writes the original as `Display`
  already does.

### 8. Work that finishes in the poll its bound fires in counts as done

- **Design:** §3.9 says `timeout(d, fut)` is a select over `sleep`; it is silent on a tie.
- **B:** `timeout` polls `fut` before the sleep.
- **Why:** the item completed; reporting `TimedOut` for a result already in hand discards it.

## Requests to other areas

### R1. Area C: report `DuplicateReadiness` from `BindingRecord::replaced_ready`

- **Item:** a new field, `BindingRecord::replaced_ready: Vec<&'static Location<'static>>`, in
  `binding/mod.rs`. `BindingRecord::new` initialises it empty; no other field changed.
- **Request:** step 2 reports `WiringError::DuplicateReadiness` when it is non-empty, with `first`
  its first entry and `second` its second entry, or `ready`'s location when it has one entry.
- **Why:** spine entry 6 makes a second `.ready(..)` a wiring error, and `ready: Option<ReadyRecord>`
  holds one check. The handle keeps the check written last in `ready`, so `.retries`, `.timeout`
  and the rest after the second `.ready(..)` write the item the user wrote last.

### R2. Area C: what `ModuleDef` sets on a `Construct` record

- **Request (coordination):** `provide::<T>()`, `provide_with`, `try_provide_with` and
  `controller::<C>()` set `hooks = erase_trait_hooks::<T>(location)` and `constructs = true`.
  `provide` and `controller` also set `construct_bound = T::CONSTRUCT_TIMEOUT`; the
  `provide_with` forms leave it `Default` for the handle to write (spine entry 4).
  `Contribute::provide` in `binding/contribute.rs` is the worked pattern.
- **Request (coordination):** an alias record is `ScopeKind::Transient` with no sites (entry 6).
  Steps 3 to 5 take its edge from `Recipe::Alias { target }`, and `connect` builds nothing for it.

### R3. Area E: what the readiness record already encodes

- **Request (coordination):** `ReadyRecord::attempt` already carries §9.3's opt-out. `.timeout(..)`
  on a check whose attempt bound is unwritten writes `Unbounded` there, and `.unbounded()` writes
  both. `run_readiness` resolves `attempt` with `BoundKind::ReadinessAttempt` and `whole` with
  `BoundKind::ReadinessWhole` through `resolve_bound`, with no rule of its own about which was
  written. `retries` counts attempts after the first (entry 2).
- **Request (coordination):** a destroy, before-shutdown or shutdown `HookFn` can return `Err`
  (entry 3). `run_hook_step` stores it as `Errored` like an init hook's, pending the user's call on
  entry 3, and does not assume a shutdown hook's `Err` is unreachable.
- **Open for E:** `.backoff(..)` on an app with no `Timer`. §10.1 step 6 does not list it among the
  bounds that need one, and nothing can wait without one.

### R4. Areas A and D: `into_primary` on every contribution

- **Request (coordination):** every contribution record carries `into_primary: Some(..)`.
  `Contribute::value` carries the identity coercion, its value being an `Arc<U>` already. A single
  binding carries `None`.

## Requests from other areas, folded in

- **A's R3** (`coercion` handed an instance that is not an `Arc<T>`): the closure returns the
  instance unchanged and does not panic. The comment at the closure states the resolver's use.

## Changes to code the spine wrote in B's files

- The sealed `handle::sealed::State` trait answers `write(record, bound)` in place of
  `bound(record) -> &mut Bound`. The hook and readiness items live in an `Option` and a `Vec`, and
  a `&mut Bound` into either needs a fallible lookup; `write` skips an absent item where `bound`
  would have had to panic. The bodies of `Handle::timeout` and `Handle::unbounded` call it, and
  lose a `mut` they no longer need. No public signature changed.
- The `HookFn` doc comment in `hooks.rs` now says when a shutdown hook's erased form answers `Err`
  (entry 3).
