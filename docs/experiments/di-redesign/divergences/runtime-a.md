# Divergences: runtime neutrality, stage a: `Spawn`, `Runtime`, `TaskHandle` and `spawn_with` in the core, `TaskSet` in `ulo-transport`, the app's runtime and `AppHandle`'s accessors, `ulo_tokio::Tokio`, the runtime conformance suite

The thirty-fourth response, signed off 2026-10-08, splits runtime neutrality into six stages; this
is the first. The core gains `Spawn`, `Runtime: Timer + Spawn` with a blanket impl, `TaskHandle`
with `abort`, a `TaskEnd` future and a detaching drop, and `spawn_with` over it. A runtime builds a
handle through `TaskHandle::launch`, which wraps the future in the core's own panic catch, so
`Panicked` is reported the same way on every runtime; the runtime implements `RuntimeTask`, three
methods, and reports only whether the task is over. `ulo-transport` gains `TaskSet`. The builder
takes `.runtime(r)`, which binds `Dep<dyn Runtime>` and makes `Dep<dyn Timer>` the same object;
`AppHandle` gains `timer()` and `runtime()`, which closes batch 18's S10 at the core.
`ulo_tokio::Tokio` implements both halves. `ulo-runtime-conformance` holds the semantics as a
macro, run here against `Tokio`. No transport changes; `ulo` and `ulo-transport` have no tokio in
their normal tree.

Files changed: `crates/ulo/src/{runtime.rs (new), lib.rs, app/mod.rs, app/handle.rs, app/load.rs,
graph/wire.rs, error/wiring.rs, testing.rs, lifecycle/run.rs}`; `crates/ulo-transport/{Cargo.toml,
src/lib.rs, src/task_set.rs (new)}`; `crates/ulo-tokio/{Cargo.toml, src/lib.rs, tests/app.rs (new),
tests/conformance.rs (new)}`; `crates/ulo-runtime-conformance/` (new: `Cargo.toml`, `src/lib.rs`,
`src/cases/{mod.rs, task.rs, value.rs, set.rs, app.rs}`); the workspace `Cargo.toml` and
`Cargo.lock`; F360 filed in the workspace's `FRAMEWORK_GAPS.md`. The tree is `bb0ac4ac` plus this
stage.

## The signatures

```rust
// ulo (crates/ulo/src/runtime.rs)
pub trait Spawn: Send + Sync + 'static {
    fn spawn(&self, fut: BoxFuture<'static, ()>) -> TaskHandle;
}
pub trait Runtime: Timer + Spawn {}
impl<R: Timer + Spawn + ?Sized> Runtime for R {}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TaskEnd { Finished, Aborted, Panicked }

pub trait RuntimeTask: Send + 'static {
    fn abort(&mut self);
    fn poll_ended(&mut self, cx: &mut Context<'_>) -> Poll<()>;
    fn detach(self: Box<Self>);
}

pub struct TaskHandle { /* private */ }
impl TaskHandle {
    pub fn launch<T, S>(fut: BoxFuture<'static, ()>, start: S) -> TaskHandle
    where T: RuntimeTask, S: FnOnce(BoxFuture<'static, ()>) -> T;
    pub fn abort(&self);
}
impl Future for TaskHandle { type Output = TaskEnd; }
impl Drop for TaskHandle { /* detaches */ }

pub fn spawn_with<T, F, R>(runtime: &R, fut: F) -> ValueHandle<T>
where T: Send + 'static, F: Future<Output = T> + Send + 'static, R: Spawn + ?Sized;
pub struct ValueHandle<T> { /* private */ }
impl<T> ValueHandle<T> { pub fn abort(&self); }
impl<T> Future for ValueHandle<T> { type Output = Result<T, TaskEnd>; }

// ulo::AppBuilder, ulo::testing::TestApp<S>
pub fn runtime(self, runtime: impl Runtime) -> Self;              // TestApp: -> TestApp<Settled>

// ulo::AppHandle
pub fn timer(&self) -> Option<&Arc<dyn Timer>>;
pub fn runtime(&self) -> Option<&Arc<dyn Runtime>>;

// ulo::WiringError (#[non_exhaustive]), a new variant
RuntimeOverride { at: &'static Location<'static> },

// ulo_transport
pub struct TaskSet { /* private */ }
impl TaskSet {
    pub fn new(runtime: Arc<dyn Spawn>) -> TaskSet;
    pub fn spawn<F: Future<Output = ()> + Send + 'static>(&mut self, fut: F);
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
    pub fn abort_all(&self);
    pub async fn join_next(&mut self) -> Option<TaskEnd>;
    pub async fn join_all(&mut self);
}
impl Drop for TaskSet { /* aborts every task still in it */ }

// ulo_tokio
#[derive(Clone, Copy, Debug, Default)]
pub struct Tokio;   // impl ulo::Timer, impl ulo::Spawn

// ulo_runtime_conformance (publish = false)
pub trait Harness {
    type Runtime: Runtime;
    fn runtime() -> Self::Runtime;
    fn block_on<F: Future>(fut: F) -> F::Output;
}
macro_rules! runtime_suite { ($harness:ty) => { /* one #[test] per scenario */ } }
pub mod cases;   // the scenarios, PATIENCE (5 s) and SETTLE (100 ms)
```

`AppBuilder::timer`'s signature is unchanged; it now also clears a runtime set before it
(decision 6). `ulo_tokio::Timer`, `spawn`, `spawn_in` and `shutdown_signal` are unchanged. Private
to `ulo`: `AppConfig::runtime`, `WireEnv::runtime`, `add_timer_module`'s `runtime` parameter, and
`CatchUnwind` taking `F: ?Sized` with a `boxed` constructor. No handler, module or controller
signature changes. `futures-util` becomes a dependency of `ulo-transport`, already in the
workspace; `ulo`'s dependencies are unchanged, `async-lock` and `ulo-macros`.

## Decisions

### 1. The panic catch is the core's own `CatchUnwind`

- **Where:** `TaskHandle::launch` wraps the spawned future in `CatchUnwind`
  (`crates/ulo/src/lifecycle/run.rs`), the poll-boundary `catch_unwind(AssertUnwindSafe(..))` the
  hook runner and `dispatch` already use. It gains `F: ?Sized` and `boxed(Pin<Box<F>>)`, so a
  `BoxFuture` is wrapped without boxing it again.
- **Not futures-util:** `FutureExt::catch_unwind` is runtime-free, but `ulo` does not depend on
  `futures-util`, and the core already had the wrapper. `ulo`'s manifest is unchanged.
- **The payload** is dropped inside the task. `TaskEnd::Panicked` carries nothing; the runtime's
  panic hook has printed the panic by then. See S6.

### 2. The SPI: `RuntimeTask`, and `launch` as the only constructor

- **Shape:** a runtime's `Spawn::spawn` calls `TaskHandle::launch(fut, |task| ..)`; the closure
  spawns the wrapped future it receives and answers its own `RuntimeTask`. `launch` is the one way
  to build a `TaskHandle`, so a runtime cannot skip the panic catch.
- **What the runtime reports:** whether the task is over, through `poll_ended -> Poll<()>`. The
  wrapper records `Finished` or `Panicked` in an `AtomicU8` before the spawned future completes
  (`Release`); the handle reads it once `poll_ended` is `Ready` (`Acquire`) and answers `Aborted`
  when nothing was recorded. A runtime that reports a panic itself and one that re-raises it on
  await give the same answer, because the panic never reaches either.
- **Locking:** `abort(&self)` is the handle's, `abort(&mut self)` the runtime's; the handle holds
  the `RuntimeTask` in a `Mutex`, so an implementer needs no interior mutability. `poll` reaches it
  through `get_mut`.
- **Drop:** the handle always calls `detach`, on a task in any state, and nothing after it.
- **The docs** on `RuntimeTask` state what each method must mean for an outside implementer and
  map them onto tokio and smol (decision 3).
- **A panic in the future's drop during an abort** is reported `Aborted`: tokio reports it as
  `JoinError::panic`, which `poll_ended`'s `()` does not carry, and nothing was recorded. See S6.

### 3. `poll_ended` waits for the future's drop

- **The rule:** `poll_ended` is `Ready` only once the future will never be polled again and has been
  dropped. A `TaskSet` waiting for all relies on no part of any task running afterwards, which is
  what the drain in stages c and d needs of `join_all` after `abort_all`.
- **tokio meets it:** an aborted task's future is dropped before its output is stored and the
  `JoinHandle` woken (`tokio-1.53.2/src/runtime/task/harness.rs:500-507`, `cancel_task`); a
  returned future is dropped when its output replaces it. `abort_is_aborted_once_the_future_is_dropped`
  holds a runtime to it through a guard whose drop sleeps 20 ms before it counts.
- **smol, as the docs describe it:** dropping a `Task` cancels it but can return while the future is
  still being polled on another thread, so the response's "`abort` drops it" does not meet the rule
  alone; the docs have `abort` keep the future `Task::cancel` answers, poll it once at once, since
  `cancel` is an `async fn` that cancels on its first poll, and `poll_ended` poll it to its end. Read
  from `async-task`'s source shape, not built; stage e's run of the same scenario decides it. See S1.

### 4. `spawn_with` leaves its value in a slot, not a channel

- **Mechanism:** the spawned future writes its value into an `Arc<Mutex<Option<T>>>` before it
  returns; `ValueHandle` awaits the `TaskHandle` and takes the value once the end is `Finished`. The
  handle's own completion is the wake-up, so no oneshot is needed and the core takes no channel
  dependency. The value is written before the wrapper records `Finished`, so a `Finished` end
  always finds it. See S2.
- **A free function**, `ulo::spawn_with(&runtime, fut)`, generic over `R: Spawn + ?Sized`, so it
  takes a `Tokio`, a `dyn Runtime` behind a `Dep` or a `dyn Spawn`; `Spawn` is not widened.
- **Its answer** is `Result<T, TaskEnd>`, the `Err` being `Aborted` or `Panicked`. Polled again after
  `Ok`, it answers `Err(TaskEnd::Finished)`, the value having been taken; the doc says so. See S2.

### 5. `TaskSet`

- **What the hubs do with `JoinSet`** (`crates/ulo-rpc/src/server.rs:199-230`, `dispatch.rs:131-558`,
  `crates/ulo-ws/src/connection.rs:811-990`): `spawn` from the serve loop and from functions taking
  `&mut JoinSet`; `join_next` as a `select!` branch guarded by `!is_empty()`, which only reaps; after
  the inbound stream ends in RPC, `join_next` until `None`; `shutdown().await` in `abandon` and at the
  end of WebSocket's connection loop. `TaskSet` covers each: `spawn`, `join_next` (cancel-safe, so
  it stands as a branch that runs again), `is_empty`, `join_all`, and `abort_all` then `join_all` for
  `shutdown`. No `shutdown` of its own: the response named abort-all and wait-for-all.
- **Storage:** a `FuturesUnordered<TaskHandle>` (`futures-util`, runtime-free), which polls only the
  handles that woke; `abort_all` walks it through `iter()`.
- **Its runtime:** held as `Arc<dyn Spawn>`, given once to `new`; an `Arc<dyn Runtime>` coerces to it.
  A task runs from `spawn` on whether the set is polled or not.
- **Drop aborts** every task still in it, as `JoinSet`'s does, which the hubs' serve futures rely on
  when they are dropped; a lone `TaskHandle` detaches. The two differ by design and the docs of each
  say so. See S3.

### 6. The app: `.runtime(r)` beside `.timer(..)`

- **Config:** `AppConfig` gains `runtime: Option<Arc<dyn Runtime>>`. `.runtime(r)` sets it and sets
  `timer` to the same `Arc`, upcast to `Arc<dyn Timer>` (trait upcasting, stable since 1.86).
  Everything that reads `timer` is unchanged, so an app given a runtime has every timed behaviour an
  app given a timer has, and an app with neither keeps today's: `Bound::Default` unbounded, a drain
  window of zero, `listen()` refusing a transport with `TimerMissing`.
- **Last call wins:** `.timer(t)` after `.runtime(r)` replaces the clock and clears the runtime,
  leaving no `Dep<dyn Runtime>`; `.runtime(r)` after `.timer(t)` replaces the timer. The two bindings
  never disagree about the clock. See S4.
- **The binding:** the core's global `TimerModule` gains a second value binding and export under
  `dyn Runtime` when a runtime is set, its label `AppBuilder::runtime` in place of
  `AppBuilder::timer` (`crates/ulo/src/graph/wire.rs`, `add_timer_module`). A lazily loaded module
  sees it as it sees the timer; `load` passes the runtime into its `WireEnv` too.
- **Overrides:** `override_value::<dyn Runtime>` is refused as `override_value::<dyn Timer>` is, as a
  new `WiringError::RuntimeOverride` with the hint "set it with `TestApp::runtime(..)`", since an
  override would split the one object. `TestApp::runtime` mirrors `TestApp::timer`.
- **Not done:** `listen()` does not refuse an app without a runtime; no transport reads one yet.

### 7. The accessor goes on `AppHandle`

- **Where:** `AppHandle::timer()` and `AppHandle::runtime()`, each `Option<&Arc<..>>` read from the
  app's config, the same objects `Dep<dyn Timer>` and `Dep<dyn Runtime>` resolve to. A link receives
  `&AppHandle` in `Link::prepare`; an embed holds the app's handle; a backend's `Mounted::app()` is
  one. Each reaches the runtime the way it now reaches the timer, without the singleton lookup
  `app.get::<dyn Runtime>().await`, which also works.
- **`Mounted`** is unchanged: `Mounted::timer()` is non-optional because `listen()` refuses an app
  without a timer, and no such refusal exists for a runtime, so a server reads
  `mounted.app().runtime()`.
- **S10 of batch 18** is closed at the core: a link can now reach the timer. Kafka's backoff is not
  changed; it runs inside `spawn_blocking`, where an async timer cannot be awaited in any case.

### 8. `ulo_tokio::Tokio`

- **A unit struct**, so `.runtime(ulo_tokio::Tokio)` compiles as written. Its `Timer` half delegates
  to `ulo_tokio::Timer`; its `Spawn` half is `TaskHandle::launch(fut, |task| Task(tokio::spawn(task)))`,
  with `Task(JoinHandle<()>)` as the `RuntimeTask`: `abort` is `JoinHandle::abort`, `poll_ended`
  polls the `JoinHandle`, `detach` drops it.
- **Ambient:** `tokio::spawn` spawns on the runtime current at the call and panics outside one; the
  doc says so. Nothing spawns from a drop yet; stage c's `RpcClient` will, and answer 5 has it spawn
  on the runtime it holds. Filed as F360.

### 9. The conformance crate names no runtime

- **Shape:** `Harness` gives a runtime value and a `block_on`; `runtime_suite!` stamps one `#[test]`
  per scenario, each calling `block_on(case(runtime()))`. The crate depends on `ulo`, `ulo-transport`
  and `futures` (its channels); a runtime crate's harness brings the executor, so stage e's smol
  harness needs nothing from tokio.
- **Waits:** every wait a scenario makes races the runtime's own `Timer::sleep`, 5 s for what is
  expected and 100 ms for what is not, so a runtime that loses a task fails rather than hangs.
- **The `Dropped` guard** sleeps 20 ms in its drop before counting, so a handle that answers while a
  drop is under way reads the count unchanged (decision 3).
- **The app's binding** is one scenario, since a smol runtime goes through `.runtime(..)` the same
  way; the `.timer` orderings and the override refusal are `ulo-tokio`'s own tests, being the
  core's behaviour rather than a runtime's.

## The tests

- **`crates/ulo-tokio/tests/conformance.rs`, twelve scenarios** from `runtime_suite!(OnTokio)`, each
  on a multi-thread tokio runtime of its own:
  - `a_returned_future_is_finished`: `Finished`, after the future ran.
  - `a_dropped_handle_detaches`: the task waits on a gate opened after its handle is dropped and
    must then report its end through a oneshot; a dropped future fails it at once.
  - `abort_is_aborted_once_the_future_is_dropped`: aborted at an await after it started; `Aborted`,
    and the guard's drop counted when the handle answers.
  - `a_panic_is_panicked_and_goes_no_further`: a panic after an await gives `Panicked`, awaiting the
    handle returns, and the next task spawned finishes.
  - `an_end_is_kept_and_abort_after_it_changes_nothing`: polled again after `Finished`, then after an
    `abort`, the handle answers `Finished` both times.
  - `spawn_with_answers_the_value`, `spawn_with_answers_an_abort`, `spawn_with_answers_a_panic`:
    `Ok(7)`, `Err(Aborted)`, `Err(Panicked)`.
  - `abort_all_aborts_every_task`: three started tasks waiting forever; after `abort_all`,
    `join_next` yields `Aborted` three times then `None`, three drops counted, the set empty.
  - `join_all_returns_once_every_task_has_ended`: three gated tasks, two opened; `join_all` must not
    return within 100 ms, then returns within 5 s once the third is opened, all three ended.
  - `a_dropped_set_aborts_its_tasks`: three started tasks waiting forever; after the set is dropped,
    all three guards drop within 5 s.
  - `the_app_binds_the_runtime_as_one_object`: an empty app given `.runtime(..)`; `Dep<dyn Runtime>`,
    `Dep<dyn Timer>`, `AppHandle::runtime()` and `AppHandle::timer()` at one address, a task spawned
    through the `Dep` finished, the app closed.
- **`crates/ulo-tokio/tests/app.rs`, five tests:** `a_timer_alone_binds_no_runtime` (`Dep<dyn Runtime>`
  is `NotFound`, `runtime()` `None`, `timer()` `Some`), `an_app_without_a_clock_hands_out_neither`,
  `a_timer_after_a_runtime_replaces_it`, `a_runtime_after_a_timer_replaces_it` (one object), and
  `overriding_the_runtime_is_refused` (`WiringError::RuntimeOverride`).

### Before and after

Each break rewrote one span through a script that asserted the span occurred once, ran the named
tests, and wrote the file back byte for byte, checked by hash; the full output of every run is in
the scratchpad (`runtime-a/broken/`, `summary.txt` beside them). Every run reported its tests by
name.

| Break | Tests run | Result |
| --- | --- | --- |
| `TaskHandle`'s drop aborts, then detaches | `a_dropped_handle_detaches` | failed: "dropping the handle stopped the task: its future was dropped unfinished" |
| `launch` awaits the future with no `CatchUnwind` | both panic scenarios | 2 failed: `Some(Err(Aborted))` for `Panicked`; tokio reported the panic as `JoinError` |
| a caught panic recorded as `FINISHED` | both panic scenarios | 2 failed: `Some(Err(Finished))` for `Panicked` |
| a return recorded as `RUNNING` | `a_returned_future_is_finished`, `spawn_with_answers_the_value` | 2 failed: `Aborted` for `Finished`, `Err(Aborted)` for `Ok(7)` |
| tokio's `RuntimeTask::abort` a no-op | the four abort scenarios | 4 failed: each wait for the end ran out at 5 s, "0 of 3 tasks were dropped" for the set |
| the handle answers on `abort` without `poll_ended` | `abort_is_aborted_once_the_future_is_dropped` | failed: "the handle answered before the task's future was dropped", left 0 |
| the end not cached | `an_end_is_kept_and_abort_after_it_changes_nothing` | failed: tokio's own panic, "JoinHandle polled after completion" |
| `spawn_with` drops the value | `spawn_with_answers_the_value` | failed: `Some(Err(Finished))` for `Some(Ok(7))` |
| `abort_all` a no-op | `abort_all_aborts_every_task`, `a_dropped_set_aborts_its_tasks` | 2 failed: the wait for an end ran out; "0 of 3 tasks were dropped after their set was" |
| `join_all` returns after one end | `join_all_returns_once_every_task_has_ended` | failed: "`join_all` returned with a task still running: 2 of 3 ended" |
| `TaskSet`'s drop empty | `a_dropped_set_aborts_its_tasks` | failed: "0 of 3 tasks were dropped after their set was" |
| no `dyn Runtime` binding | `the_app_binds_the_runtime_as_one_object` | failed: `NotFound { key: dyn Runtime, kind: Binding }` |
| `.runtime(..)` gives the timer a wrapper of its own | same | failed: "`Dep<dyn Timer>` is another object than `Dep<dyn Runtime>`" |
| `AppHandle::runtime` answers `None` | same | failed: "`AppHandle::runtime`", left `None` |
| `.timer(..)` keeps the runtime | `a_timer_after_a_runtime_replaces_it` | failed: "`Dep<dyn Runtime>` after `.timer(..)` replaced the runtime" |
| the `dyn Runtime` override not refused | `overriding_the_runtime_is_refused` | failed: "an override of `dyn Runtime` wired" |
| the restored tree, three runs | `cargo test -p ulo-tokio --locked` | 12 and 5 passed each; the suite 0.10–0.11 s |

The guard's 20 ms pause was in place for every break run; the "answers before the drop" break was
not run without it.

## Left for the transports DESIGN fold

- §0, principle 5 (line 13): "`fw-tokio` supplies the core's `Timer`, signals, and spawn helpers"
  gains the core's `Runtime`; the principle that transports may depend on tokio is what the later
  stages retire.
- §1's crate table, `fw-tokio`'s row (line 43): `Tokio` beside `Timer`; the "uses" column gains
  `Runtime`, `Spawn` and `TaskHandle`. A row for `fw-runtime-conformance`, `publish = false`, a
  dev-dependency of the runtime crates.
- §8 (line 1137): "`fw-tokio` provides three things" becomes four: `Tokio`, the core's `Runtime`
  over `tokio::spawn` and `JoinHandle`, spawning on the runtime current at the call (F360).
- §11's SPI table (line 1178): a row for `Spawn`, `Runtime`, `TaskHandle`, `TaskEnd`, `RuntimeTask`,
  `spawn_with` and `ValueHandle` in the core, `TaskSet` in `fw-transport`, `AppHandle::timer` and
  `AppHandle::runtime`, and `AppBuilder::runtime` and `TestApp::runtime`; core DESIGN's surface
  paragraph (line 1370) names the same additions.
- Core DESIGN §3.9 (lines 366-380): the trait block gains `Spawn` and `Runtime`; the binding
  paragraph (line 380) gains `dyn Runtime` beside `dyn Timer`, one object, and its override refusal.
  §9.4 (line 858 on) gains `.runtime(r)` on the builder and its last-call-wins rule against
  `.timer(t)`. §8.6's `AppHandle` paragraph (line 777) gains `timer()` and `runtime()`. §10.1 step 2
  (line 933), §11's `Timer` bullet (line 1229) and its refusal table (line 1271) gain
  `override_value::<dyn Runtime>` with the hint "set it with `TestApp::runtime(..)`".
- Transports §1's `fw-hyper-serve` row (line 27) and §3.7's accept-loop paragraph (line 541), both
  naming `JoinSet`, and §5.4's client paragraph (line 987), "from a spawned task, and not at all
  outside a tokio runtime", stand until stages b and c move them.

## Needs sign-off

### S1. `poll_ended` waits for the future's drop, which smol's plain drop does not give

Decision 3: the rule tokio already meets, and the one a drain after `abort_all` needs. The response
had smol's `abort` drop the `Task`; meeting the rule, the docs have it start `Task::cancel` and
`poll_ended` finish it. The alternative is the weaker rule, `poll_ended` `Ready` once the runtime
has accepted the abort, with a drain that cannot say no part of a task still runs.

### S2. `spawn_with` without a channel, answering `Result<T, TaskEnd>`

Decision 4: the response said a helper over a oneshot channel; the slot read after the handle's own
end does the same with no dependency. The answer's `Err` can only be `Aborted` or `Panicked` but is
typed `TaskEnd`, and a second poll answers `Err(Finished)`. The alternatives are `futures-channel`'s
oneshot in the core, and an error enum of two variants.

### S3. A dropped `TaskSet` aborts, a dropped `TaskHandle` detaches

Decision 5: the set aborts on drop as the `JoinSet` it replaces does, which the hubs' serve futures
rely on when the core drops them. The response fixed detach-on-drop for the handle and did not say
for the set. The alternative is a set that detaches, with every owner aborting explicitly.

### S4. `.timer(..)` after `.runtime(..)` clears the runtime

Decision 6: the later call wins, so the two bindings never disagree about the clock. A test pairing
`Tokio`'s spawn with a mock clock writes a `Runtime` of its own. The alternatives are a wiring error
for both set, or keeping the runtime with `Dep<dyn Timer>` a different object from its clock.

### S5. A new `WiringError::RuntimeOverride`

Decision 6: an override of `dyn Runtime` would split the one object, so it is refused as the timer's
is. The variant is new on a `#[non_exhaustive]` enum. The alternative is to let the override stand,
`Dep<dyn Runtime>` then differing from the clock the core times with.

### S6. `TaskEnd` carries no panic payload, and a panic during an abort's drop reads `Aborted`

Decisions 1 and 2: the payload is dropped inside the task, and `poll_ended`'s `()` cannot carry a
panic the runtime saw while dropping an aborted future. The alternatives are a payload or its
message on `Panicked`, and `poll_ended` answering whether the runtime saw a panic.

### S7. The accessor is on `AppHandle` alone

Decision 7: `Mounted` keeps its non-optional `timer()`, and a server reads the runtime through
`mounted.app().runtime()`. The alternative is `Mounted::runtime()`, non-optional once `listen()`
refuses an app without a runtime, which is a stage-c decision.

## Verification

Full, unfiltered output of every run is in the session scratchpad, `runtime-a/`.

- **The tree checks,** each first run against a violation: with `tokio = { workspace = true,
  features = ["rt"] }` appended to `crates/ulo/Cargo.toml`, `cargo tree -p ulo -e normal
  --all-features -i tokio` printed `tokio v1.53.2` / `└── ulo v0.2.0`, and the same for
  `ulo-transport` printed the path through `ulo` (`tree-violation.txt`). The manifest and
  `Cargo.lock` were restored from copies, checked by hash. On the final tree both print nothing on
  stdout and exit 101 with "package ID specification `tokio` did not match any packages"
  (`tree-final.txt`, `tree-final-stdout.txt`). `ulo-runtime-conformance`'s normal tree has no tokio
  either.
- `cargo check --workspace --all-targets --all-features` with
  `CFLAGS=-I/opt/homebrew/opt/openssl/include LDFLAGS=-L/opt/homebrew/opt/openssl/lib`: exit 0, the
  17 known warnings in `crates/ulo/src` and no warning location elsewhere.
- `cargo +1.88 check --workspace --all-targets --exclude ulo-http-salvo --exclude
  ulo-graphql-async-graphql`: exit 0, the same 17 and no other. `.runtime(..)`'s upcast from
  `Arc<dyn Runtime>` to `Arc<dyn Timer>` needs 1.86.
- `cargo test --workspace --no-fail-fast` with the OpenSSL flags: 463 passed, 0 failed, 65 ignored
  across 125 test binaries: batch 18's 446, the twelve scenarios and five app tests in `ulo-tokio`.
  The binaries added are those two, `ulo-runtime-conformance`'s unit tests (none) and its doc test,
  the `ignore` example, which is the ignored one added.
- `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --lib -p <crate>` for `ulo`, `ulo-transport`,
  `ulo-tokio` and `ulo-runtime-conformance`: each passes.
- `cargo +1.98.1 clippy -p ulo -p ulo-transport -p ulo-tokio -p ulo-runtime-conformance
  --all-targets --no-deps --locked`: exit 0, 78 warning locations, none on a line this stage wrote
  or in a new file, checked against `git diff -U0`'s hunks (`clippy-locations.txt`). The check was run
  again over the same output with two planted locations, one on a changed line of `app/handle.rs`
  and one in `runtime.rs`, and reported both (`checker-selftest/`).
- After the workspace run and before the doc and clippy runs, two lines of `ulo-tokio`'s
  `tests/app.rs` and one of the suite's `cases/value.rs` were wrapped, and after the clippy run two
  doc sentences in the suite were reworded, nothing else; `cargo test -p ulo-tokio --locked` passed
  again, 12 and 5, `cargo +1.88 check -p ulo-runtime-conformance -p ulo-tokio --all-targets` exited
  0, and the suite's doc built under `-D warnings`.
- No container was started, and no broker suite was run: no transport changed. `docker ps` before
  and after the workspace run listed the user's four, seaweedfs, mailpit, postgres:18 and redis:7,
  and nothing else.
