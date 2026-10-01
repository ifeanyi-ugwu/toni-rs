# Divergences: area D (executions, the `App` typestates, `AppHandle`, `execute`, `load`, `ModuleRef`, `TestApp`)

Every place area D's code departs from `DESIGN.md`, or fills a gap the design leaves that a user
would see. Each entry gives what the design says, what D wrote, and why. All await the user's
sign-off. Requests for other areas, and how D answered the requests made of it, follow the entries.

## Entries

### 1. How `execute` enforces a standalone deadline

- **Design:** §3.8: "a standalone execution's fires at its deadline"; "The core stores the
  deadline, and the transport's runtime enforces it." `BUILD_PLAN.md` leaves the mechanism to D.
- **D:** `execute` computes `deadline - timer.now()` on the app's `Timer` and polls the closure's
  future and `Timer::sleep` of that length in one `poll_fn`, the sleep first. When the sleep
  resolves, cancellation fires and the closure is polled on until it returns. A deadline already
  past fires cancellation before the closure first runs. With no `Timer` the deadline is stored,
  and `deadline()` reports it, but cancellation never fires for it. `Execution::open` stores a
  deadline and never enforces it: whoever holds the execution does, a transport for a call.
- **Why:** the `Timer` is the core's only clock; reading `std::time::Instant` against a paused
  test clock is the mismatch §3.9 rules out. Cancellation is a signal to the closure, not the end
  of its future, so the closure keeps running (§3.8). `execute` answers `Result<R, Closed>` and has
  no error to refuse a deadline it cannot time.

### 2. `serve` drives each transport's `Server::serve`, and a transport failure is a trigger

- **Design:** §9.4 and §9.5 name two triggers and say nothing about who polls a transport's
  accept loop or what a transport failing does. The spine's `Server::serve` doc says "An error
  before the drain starts the shutdown."
- **D:** `App<Bound>::serve` polls every bound transport's `serve` future beside its signal and,
  once a trigger has arrived, beside the shutdown sequence, until the sequence's outcome. A
  `serve` that fails before any trigger starts the shutdown under
  `Signal::new("transport `<name>` failed: <error>")`, the error passed through the redaction
  function first. That signal is the only place the failure reaches the outcome. A `serve` that
  returns `Ok` early is done and triggers nothing; a failure after a trigger is dropped.
- **Why:** a transport keeps serving through the before-shutdown stage and the drain, so its loop
  has to be polled until the sequence ends. `Shutdown` and `ShutdownError` have no field for a
  serve failure, and `Shutdown::signal` already names what ended the app.

### 3. `listen` with several transports

- **Design:** §9.5 and §12 refuse an app that binds a transport with no `Timer`; silent on which
  transport the refusal names when several are queued, and on sockets taken before a later bind
  fails.
- **D:** with no `Timer`, `listen` refuses before binding anything, naming the first transport
  queued. Otherwise the transports bind in the order queued; when one fails, those already bound
  are closed, their close errors ignored, and the bind error returns as `StartupError::Bind`.
- **Why:** a refused app should hold no sockets, and `NoTimer` is a property of the app rather
  than of one transport, so the first is as good a name as any.

### 4. A failed `load` leaves the app as it found it

- **Design:** §8.6 lists `load`'s errors; silent on the state a `Connect` failure leaves.
- **D:** the extended graph is published before its connect walk, since the new constructors
  resolve through the app's graph. When the walk fails, the base graph is put back and the
  singletons the load built are removed from the store, so a later `load` of the same module wires
  again instead of answering the half-loaded module's handle. Init hooks that ran before the
  failure get no destroy hook. Loads are serialized, and `load` checks the phase again after
  waiting on another load.
- **Why:** an identity left in the graph would make the retry return a module whose singletons
  were never built. Running destroy hooks for part of a module is a policy the design does not
  state; entry 5 leaves the same gap at startup.

### 5. A failed `connect` tears nothing down

- **Design:** §9.2: the first failure stops the walk. Silent on what was already built.
- **D:** `App::connect` returns `StartupError::Connect` and drops the app. Singletons already
  built and init hooks already run get no destroy or shutdown hook; their resources are released
  only as the instances drop.
- **Why:** none of §9.5's steps is defined for an app that never reached `Connected`. Running
  them for a partial graph is a choice the user should make.

### 6. No override reaches a binding written with `.qualified::<Q>()`

- **Design:** §11's overrides name a type: `override_value::<T>`, scoped with `in_module*`.
  Spine entry 11 lets a binding be qualified outside a keyed module.
- **D:** every `override_*` targets `T @ ()`. Inside a keyed module that is the binding's own key,
  which `in_module_keyed::<M, Q>()` reaches. A binding qualified `T @ Q` with `.qualified::<Q>()`
  has no override spelling.
- **Why:** the public surface is frozen. An `override_value_qualified::<T, Q>` beside the others
  would close the gap.

## Requests for other areas

### R1. Area C: read `Override::sites`

- **Item:** a new field, `Override::sites: Sites`, in `crates/ulo/src/testing.rs`. Empty for
  `override_value`; the factory's parameter sites for `override_factory` and
  `override_try_factory`.
- **Request:** when an override is applied, its sites replace the replaced binding's sites.
- **Why:** the old sites describe a recipe that no longer runs. Without the factory's own sites,
  steps 3 to 5 cannot see its dependencies: `connect` may build it before them and fail with
  `NotReady`, and a cycle through it goes unreported. A destructuring of `Override` without `..`
  has to name the field.

### R2. Area E: what the singleton store holds (coordination)

- **Item:** `SingletonStore::insert`, from `build_singleton`.
- **Request:** store the instance as the recipe built it, not widened by `into_primary`, and hand
  that instance to the binding's hooks as `HookCx::instance`.
- **Why:** a contribution's trait hooks (`contribute::<U>().provide::<T>(..)`) downcast the
  instance to `T`. `obtain` widens on the way out. B's coercion passes an already-widened instance
  through unchanged (B's log, A's R3), so storing a widened one breaks only the hooks, not lookups.

### R3. Area E: phase transitions around `connect` (coordination)

- **Request:** `lifecycle::connect::connect` leaves the phase alone. `App::connect` advances to
  `Connecting` before it and to `Running` after it; `load` calls it while the app is `Running`.

### R4. Area E: `close` runs the sequence when its trigger wins (coordination)

- **Request:** `serve` calls `shutdown::close(&shared, signal)` for both triggers: with its own
  signal when that resolves first, and with the winning signal once `triggered()` fires. `close`
  therefore has to run `run_sequence` itself when its `trigger` wins, and join the running one
  otherwise, as its doc says.

### R5. Area C: the frozen metadata holds `T` itself (coordination)

- **Item:** `FrozenMeta`, as `ModuleRef::meta` reads it.
- **Request:** freezing converts each `MetaEntry::value` with `Arc::from(box)`, so the
  `Arc<dyn Any + Send + Sync>` holds the `T`; `meta::<T>()` downcasts it with `Arc::downcast::<T>`.

### R6. Area C: what `load` reads from `LazyWiring` (coordination)

- **Request:** the extended graph keeps every base module and binding at its id. The modules the
  load brought are ids `base.modules.len()..graph.modules.len()`, in collection order, and `load`
  passes exactly those to `connect`. `LazyWiring::singletons` lists every singleton they bind, in
  connect order. `module::<M>()` calls `Graph::find_module(.., None)` and `module_keyed::<M, Q>()`
  passes `Some(Qualifier::of::<Q>())`; what `None` matches is C's to define, §8.5 expecting
  `AmbiguousModule` over several configurations.

## Requests made of D

- **A's R1** (inputs in the `Instance` shape): done. `Inputs::insert` stores
  `instance_of(Arc::new(value))`; `get` reads with `downcast_instance`; `get_erased` returns the
  `Instance`.
- **A's R2** (what `obtain` returns): `obtain` answers in the binding's primary key type,
  applying `into_primary` whenever a record carries one, which A says its direct downcast accepts.
  An alias resolves its target with `graph.lookup(alias origin, target)` and is converted to the
  target key's type through the source's `also_as` entry, matched by `TypeId`. The recursion goes
  through a named `BoxFuture`.
- **B's R4** (`into_primary` on every contribution, the identity on `Contribute::value`):
  applied whenever present, values included.
- **F's R3**: `AppHandle::shared` and `AppConfig::timer` stay `pub(crate)`. `listen` refuses an
  app with no `Timer` before any `bind` runs. `ExecShared::resolver`, which both `Execution` and
  `ExecutionRef` call, attaches the execution.

## Additions inside D's files

- `SingletonStore::remove`, `AppShared::{get_root, find_module}`: `pub(crate)`, called by D alone.
- An execution counts in the live set from `LiveSet::enter`, before the phase check, so a drain
  that starts after the check passes waits for it. `cancel_all` marks the set, and an execution
  attached after it is cancelled at once. A refused terminal execution is counted by
  `PhaseCell::allows_execution`, which `open` calls once per execution.
