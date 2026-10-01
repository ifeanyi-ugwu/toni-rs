# Divergences: area E (lifecycle, connect, shutdown, redaction and errors)

Every place area E's code departs from `DESIGN.md`, or fills a gap the design leaves that a user
would see. Each entry gives what the design says, what E wrote, and why. All await the user's
sign-off. Requests for other areas follow the entries.

## Entries

### 1. Hook order across modules, lazily loaded modules and one binding's hooks

- **Design:** §9.2: hooks run in connect order, "a module's own hooks run after the hooks of its
  providers", and close runs the exact reverse. §8.6: lazily loaded modules shut down "in reverse
  order of loading". `BUILD_PLAN.md` leaves the order across lazy modules and the base graph to E.
- **E:** at startup a module's own hooks run right after the last of its singletons in connect
  order; a module with no singleton runs after the modules before it in collection order. Several
  hooks of one kind on one binding, a trait hook and a closure hook, run in registration order.
  Each shutdown hook step runs the lazily loaded groups first, the latest load first, then the
  base graph, each group in the exact reverse of its startup order, one binding's hooks included.
- **Why:** placing a module's hooks after its own providers, and not after every provider, keeps
  a module initialised before the modules that import it, which is NestJS's order. Shutdown is the
  mirror of startup within each group, and the lazy groups wrap the base graph the way a later
  load wraps an earlier one.

### 2. The key a hook or construction failure names

- **Design:** `ConnectError::{Construct, Readiness, Hook}` and `ShutdownFailure::Hook` carry
  `key: KeyName`; silent on module hooks, which have no binding, and on contributions.
- **E:** a module hook names its module's identity type as a single key, qualified when keyed:
  `DbModule @ Replica`. A binding names its primary key, qualified, so a contribution names its
  collection: `dyn HealthIndicator (collection)`.
- **Why:** `KeyName` needs a `Key`. A module's type is the only key a module hook has, and needs
  R1. A contribution's built type is recorded only as a type name, which no `Key` can carry.

### 3. A failed site read during `connect`

- **Design:** §10.2: a `ConstructError::Site` "is reported as the `LookupError` it carries, on the
  path that error was already taking". `ConnectError` has no variant carrying a `LookupError`.
- **E:** a nested build's `LookupError::Construct { key, reason }` becomes
  `ConnectError::Construct` with that deeper key, its module and its reason. Any other lookup
  error, such as the `NotReady` a constructor's `ModuleRef::get` meets during `connect`, becomes
  `ConnectError::Construct { key: <the binding being built>, reason: Errored(..) }`, the
  `LookupError` reachable through `downcast_ref`. In a readiness check, a site read that fails is
  an attempt's `Err` and is retried like one.
- **Why:** the nested case keeps the design's "name the deeper key". The others have no deeper
  failure to name, and a new `ConnectError` variant would change a public enum.

### 4. `Errored` on a shutdown hook

- **Design:** §10.2: `ShutdownFailure::Hook` is "never `Errored`: the shutdown hook traits return
  `()`".
- **E:** a destroy, before-shutdown or shutdown closure hook whose site read fails returns `Err`
  from its erased form (B's entry 3), and `run_hook_step` records it as `Errored`. The doc comment
  on `ShutdownFailure::Hook` says so.
- **Why:** dropping the failure would hide a hook that never ran. The user's call on B's entry 3
  decides both.

### 5. `.backoff(..)` on an app with no `Timer`

- **Design:** §10.1 step 6 lists the bounds that need a `Timer`; `.backoff` is not among them, and
  nothing can wait without one.
- **E:** the next attempt follows at once, with no wiring error.
- **Why:** `.retries` keeps its meaning and the check stays usable in a timerless job. Refusing it
  is the alternative, which would be C's step 6.

### 6. What a `Redacted`'s text holds

- **Design:** "a `Redacted` writes the redacted text"; silent on what the text is.
- **E:** the error's message, then ": " and each message down its `source()` chain that the text
  does not already contain, up to 32 links, all redacted together.
- **Why:** `Redacted::source()` answers `None`, so the text is all a reporter prints. With the
  message alone, the cause of a wrapped error, often the one naming the host or file, would
  appear nowhere.

### 7. A panic payload as a message

- **Design:** `Panicked(Redacted)`, "the payload, converted to a message".
- **E:** a `&'static str` or `String` payload is the message; any other payload reads "a panic
  whose payload is not a string". `into_inner` hands back a private error type whose `Display` is
  the unredacted message.
- **Why:** `BoxError` is `Sync` and a panic payload is not, so the payload cannot be kept. A public
  type for the inner error would add to the surface for a downcast nobody needs.

### 8. What the redaction function matches

- **Design:** it replaces every registered `Secret<_>` and strips the userinfo from anything shaped
  like a URL.
- **E:** registered texts are replaced by `[redacted]` in one pass, the longest first; an empty
  text is never registered. The strip treats `scheme://` as the start of a URL and ends its
  authority at `/`, `?`, `#`, whitespace, a quote, a backtick or an angle bracket; everything
  before the authority's last `@` becomes `[redacted]`. `register_if_secret` recognizes a
  `Secret<String>` and an `Arc<Secret<String>>`.
- **Why:** one pass keeps a short secret from matching inside an earlier `[redacted]`, and an empty
  text would match between every two characters. The terminators are RFC 3986's plus the
  characters that bracket a URL in prose. A password holding a raw `/` ends the authority early
  and is not stripped; the strip is a backstop.

### 9. `Debug` of the four reports writes their `Display` text

- **Design:** silent. §9.4's `main` returns `Result<(), Box<dyn std::error::Error>>`, which prints
  an error's `Debug`, and §10.1 shows the report as "a sample of the output".
- **E:** `StartupError`, `LoadError`, `ShutdownError` and `WiringErrors` implement `Debug` by
  writing their `Display` text, in place of the spine's derives. The other error types keep their
  derived `Debug`.
- **Why:** with the derives, §9.4's `wire()?` prints a struct dump instead of §10.1's report.

### 10. `StartupError` and `LoadError` are transparent

- **Design:** silent on their text and `source()`.
- **E:** each writes the wrapped error's text, `Bind` as ``transport `X` failed to bind: {source}``
  and `LoadError::Closed` as "cannot load a module: the application is shutting down".
  `source()` answers the wrapped error's own source, which is `None` for every wrapped type, not
  the wrapped error.
- **Why:** answering the wrapped error from `source()` while writing its text would print it twice
  in a reporter that walks the chain.

### 11. Message and report text

- **Design:** §10.1's sample gives the report's layout and five entries; silent on every other
  text.
- **E:** every `WiringError` follows the sample's layout: a headline, a `├─`/`└─` tree of details,
  the help last. Additions to the sample: `Ambiguous` lists `needed by <consumer>` before the
  sources, padded so the locations align; `ScopeViolation` names the module, and an `Auto`
  provider's headline says it is a singleton because it is a provider, with §6.2's hint. Paths
  print with ` → `; a cycle's path is closed back to its first step. `FailureReason` reads
  `panicked: {message}`, "timed out after 2s (the attempt bound)" (or "its own bound", "the app
  default", "`shutdown_timeout`"), "skipped: `shutdown_timeout` had expired", or the error's text.
  `LookupError::AmbiguousModule` reads "`X` is ambiguous between A, B", which fits both a module
  type and a binding key's type (A's entry 4).
- **Why:** the sample's form, applied to every variant. `BUILD_PLAN.md` gives the hint text to C
  while the `Display` sits in E's file; C's choices reach this file as requests.

### 12. Transports in the shutdown sequence

- **Design:** §9.5 step 2 hands each transport a `DrainToken` through `Server::drain` "at the same
  moment"; step 6 closes the sockets. Silent on whether the core waits for `drain`, and on the
  close order.
- **E:** every transport's `drain` future runs inside the drain window beside the wait for live
  executions. The window ends once every `drain` future has completed and no execution is live,
  or at its deadline, where an unfinished `drain` future is dropped. Step 6 calls `close` on each
  transport in reverse bind order, one after another, under no bound.
- **Why:** a transport that closes busy connections after their last message is still draining,
  and its terminal executions open during that time. Ending the window at the first empty live
  set would refuse them. The close runs no user code, which §9.5 exempts from the cap.

### 13. A shutdown runner dropped mid-sequence

- **Design:** silent. The sequence runs in whichever `serve` or `close` future won the trigger.
- **E:** the sequence keeps its position, the step under way and how many of its hooks or
  transports have started, in the `ShutdownCell`. If the future running it is dropped, the next
  `serve` or `close` caller resumes from that position with the same cap and drain deadline. A
  hook or `close` that had started is not run again and records nothing.
- **Why:** restarting would run hooks a second time, and leaving the outcome unset would hang every
  waiting caller.

## Requests for other areas

### R1. Area A, `key.rs`: a key from runtime parts

- **Item:** `pub(crate) fn Key::from_parts(ty: TypeId, ty_name: &'static str, qualifier: TypeId,
  q_name: &'static str) -> Key`, the constructor C's `Graph::find_module` in `graph/mod.rs`
  already calls with those four arguments.
- **Why:** entry 2. `lifecycle::connect::site_key` names a module hook's module from
  `ModuleIdentity::{type_id, type_name, qualifier}`, which no `Key` constructor takes today.
  `connect.rs` calls it and does not compile without it.

### R2. Area C: what the shutdown sequence reads from the graph (coordination)

- **Items:** `Graph::{modules, bindings}`, beyond the contract's listed fields:
  `FrozenModule::{id, identity, hooks, loaded}` and `FrozenBinding::{origin, record}`.
- **Request:** the extended graph's `connect_order` includes every singleton a lazy load brought,
  or their hooks never run at shutdown. Every module a load brought carries `loaded: Some(n)`, with
  `n` increasing from load to load.

### R3. Area C: the `WiringError` fields as the report prints them (coordination)

- `ScopeViolation::path` starts at the binding, as in §10.1's sample; the report prefixes the
  binding when it does not.
- `InputNotSeeded::path` starts at the handler or right after it; the report prefixes
  `<handler> (<transport>)` when it does not. `transport` and `seeder` are type names, shortened.
- `ImportCycle::path` and `Cycle::path` may end on their first step or not.
- `BoundWithoutTimer::item` is a lowercase phrase, printed as
  ``{item} needs a `Timer`, and the app has none``; `KnobWithoutTimer::knob` is the knob's bare name.

### R4. Area D: what E settled with D (coordination)

- `connect` leaves the phase alone (D's R3). `close` triggers and runs the sequence when its
  trigger wins, joining the running one otherwise (D's R4). The store holds an instance as built,
  and hooks receive that instance (D's R2).
- `PhaseCell::allows_execution(true)` counts each refused terminal execution for
  `Shutdown::terminal_skipped`, and refuses from the drain's end, which `PhaseCell::end_drain`
  marks before `LiveSet::cancel_all`.
- `drain` waits on `LiveSet::until_empty`, which has to resolve at once on an empty set.

### R5. Area F: `Server::drain` and `close` as the sequence drives them (coordination)

- A transport's `drain` future may run for the whole drain window and is dropped at its end;
  returning once stop-accepting has started is also correct.
- `close` is called once per transport, in reverse bind order. `transport_name()` is printed
  through `short_type_name`.

**R1 applied by the orchestrating session** (area A had finished): `pub(crate) fn Key::from_parts(ty:
TypeId, ty_name: &'static str, qualifier: TypeId, q_name: &'static str) -> Key` in
`crates/ulo/src/key.rs`, in the argument order both call sites use (`graph/mod.rs`,
`lifecycle/connect.rs`).
