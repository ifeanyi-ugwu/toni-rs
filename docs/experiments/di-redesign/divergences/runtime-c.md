# Divergences: runtime neutrality, stage c: `ulo-rpc` off tokio, the server spawning on the app's runtime, `RpcClient` given its runtime, and `ulo_tokio::Tokio` holding its `Handle`

The thirty-fourth response splits runtime neutrality into six stages; this is the third. `ulo-rpc`
has no tokio in its normal tree. Its server spawns every call into a `TaskSet` over the app's
`Runtime`, refuses in `prepare` an app with none, and races its futures through `futures-util`'s
`select`; its drain state and close signal are a private `Watch` over a `std` mutex and an
`event-listener` event; a streamed request's items and each call's reply frames cross
`futures-channel` channels; the client's connection lock is `async-lock`'s. `RpcClientModule`
reads `Dep<dyn Runtime>`. `RpcClient::new(link, runtime)` builds a client outside an app, which
owns its link and closes it from a task on its runtime when dropped; the ambient
`Handle::try_current` is gone. `ulo_tokio::Tokio` holds a `tokio::runtime::Handle`, built by
`Tokio::current()` or `Tokio::from_handle(h)`, and spawns and sleeps on that runtime from any
thread, which fixes F360. The `Link` SPI is unchanged; the seven links stay tokio-based inside.

Files changed: the workspace `Cargo.toml` and `Cargo.lock`; `crates/ulo-rpc/{Cargo.toml,
src/lib.rs, src/server.rs, src/dispatch.rs, src/client.rs, src/client_module.rs, src/transport.rs,
src/extract.rs, src/watch.rs (new), tests/runtime.rs (new), tests/drain_window.rs,
tests/stream_end.rs}`; `crates/ulo-tokio/{src/lib.rs, tests/app.rs, tests/conformance.rs,
tests/handle.rs (new)}`; `crates/ulo-runtime-conformance/src/lib.rs`;
`crates/ulo-rpc-conformance/src/cases/app.rs`; `crates/ulo-rpc-tcp/tests/{client_close.rs,
deadline_answers.rs, drain_end.rs}`; `crates/ulo-rpc-udp/tests/{client_close.rs, drain_end.rs}`;
`crates/ulo-rpc-kafka/src/link.rs` (doc only); F360 appended and F363 filed in the workspace's
`FRAMEWORK_GAPS.md`. The tree is `730d8228` plus this stage.

## The signatures

```rust
// ulo_rpc::RpcClient, a new public constructor; the crate-private `new(Arc<L>, Bound, Arc<dyn Timer>)`
// it had is now the private `of_module(Arc<L>, Bound, Arc<dyn Runtime>)`
impl RpcClient {
    pub fn new<L: Link>(link: L, runtime: Arc<dyn Runtime>) -> RpcClient;
}

// ulo_tokio::Tokio, from a unit struct to one holding a handle
#[derive(Clone, Debug)]
pub struct Tokio { /* private: tokio::runtime::Handle */ }   // was: #[derive(Clone, Copy, Debug, Default)] pub struct Tokio;
impl Tokio {
    pub fn current() -> Tokio;                 // panics outside a tokio runtime, at this call
    pub fn from_handle(handle: Handle) -> Tokio;
}
impl ulo::Timer for Tokio {}                   // enters the held runtime for `sleep` and `now`
impl ulo::Spawn for Tokio {}                   // `Handle::spawn` on the held runtime
```

`ulo_tokio::Tokio` as a value expression no longer compiles: `.runtime(ulo_tokio::Tokio)` is
written `.runtime(ulo_tokio::Tokio::current())`. `ulo_tokio::Timer`, `spawn`, `spawn_in` and
`shutdown_signal` are unchanged. `ulo_runtime_conformance::Harness` is unchanged in shape;
`runtime_suite!` now calls `Harness::runtime()` inside the future it hands `block_on`, and the
trait method's doc says so. The `Link` trait, `Inbound`, `Outbound`, `Delivery`, `ReplyPath`,
`Ack`, `Server`, `RpcClientModule`, `Call`, `Emit`, `StreamCall`, `RpcStream` and `RpcError` are
unchanged: no public type in them named a tokio type. `Body::Stream` (crate-private) holds a
`futures_channel::mpsc::UnboundedReceiver`. No handler, module or controller signature changes,
and the macros' output names no tokio item, before or after.

Dependencies: `ulo-rpc` loses `tokio` (`rt`, `sync`, `macros`) and gains `async-lock`,
`event-listener` and `futures-channel`; its tokio dev-dependency gains `rt`, `macros` and
`sync` beside `time`. The workspace gains `event-listener = "5.4"` and `futures-channel = "0.3"`,
both already in the lock (`async-lock` 3.4 depends on `event-listener` 5.4.1); `Cargo.lock` gains
the three edges and no package.

## Decisions

### 1. Each tokio use in `ulo-rpc` and what replaced it

| Use | Where | Replacement |
| --- | --- | --- |
| `JoinSet<()>` | `server.rs` `serve`; `dispatch.rs` `accept`, `start`, `unhandled`, `refuse` | `ulo_transport::TaskSet` over the app's runtime, coerced from `Arc<dyn Runtime>` to `Arc<dyn Spawn>` |
| `JoinSet::shutdown().await` | `server.rs` `abandon` | `abort_all()` then `join_all().await`; stage a's `poll_ended` rule has each abort complete with the future dropped, so a `Refusing` guard has counted down before `abandon` returns |
| `tokio::select!` biased: close, delivery, reap if non-empty | `serve`'s reading loop | `reading`: `select(closed, select(inbound.next(), reaping))`, where `reaping` awaits `join_next` while the set was non-empty at the turn's start and is pending otherwise. `futures_util::future::select` polls its first future first, so the nesting keeps the order; every loser is dropped when the turn returns, and the stream's `next` and `join_next` take nothing out until they answer |
| `tokio::select!` biased: close, next end | `serve` after the stream ended | `finishing`: `select(closed, join_next)` |
| `tokio::select!` biased: recovery, grace | `dispatch.rs` `expired` | `select(pin!(recovering), timer.sleep(grace))` |
| `tokio::select!` biased: cancellation, `fut`, deadline | `dispatch.rs` `until_ended` | `select(cancelled, select(fut, expired))`; the cancellation is still written inside the expiry future's own poll, before `fut` is dropped with the `Either` |
| `tokio::select!` unbiased | `client.rs` `bounded` | `futures_util::select!`, which picks at random among the futures ready, as tokio's unbiased form does; each arm's future is `.fuse()`d inside the macro, `bounded` running the race once |
| `watch::Sender<bool>` and `closed(&mut Receiver)` | `Server::closing` | `Watch<bool>` (decision 2); "or the server holding the sender is gone" left the doc, `serve` borrowing the server it reads |
| `watch::Sender<Settling>`: `send_modify`, `borrow`, `subscribe().wait_for` | `Shared::settling` | `Watch<Settling>`: `modify`, `read`, `wait_for` |
| `mpsc::unbounded_channel::<Data>` | a streamed request's items (`Live::inbound`, `Body::Stream`, `Inbound<T>`) | `futures_channel::mpsc::unbounded`: `unbounded_send`, and the receiver read with `StreamExt::next`; the channel closes when `in_end` drops the last sender, as before |
| `mpsc::unbounded_channel::<Result<Frame, RpcError>>` | each call's reply frames (`Waiting`, `Exchange::replies`) | the same; `UnboundedSender::is_closed` stops the pump as before |
| `tokio::sync::Mutex<Option<Arc<Conn>>>` | `ClientInner::conn`, held across `connect` | `async_lock::Mutex` |
| `tokio::spawn` | the reply router per connection, a streamed request's pump | `runtime.spawn(..)` on the client's runtime, the `TaskHandle` dropped, which detaches, as dropping a `JoinHandle` did |
| `Handle::try_current()` then `spawn` | `Pending::drop`, the `cancel` | `Conn::runtime.spawn(..)`, the connection holding the client's runtime; nothing looks for one |
| `#[tokio::test]` and `tokio::time` | tests | unchanged, dev-dependencies |

No `ulo-rpc` wait used tokio's clock outside tests: every sleep already went through the app's
`Timer`, which `Shared::timer` and the client's runtime carry.

### 2. The watch is a `std` mutex and an `event-listener` event

- **What both sites need:** a value read under a predicate, changed from synchronous code (a
  `Drop` decrements the refusals), with every waiter woken on each change. `async-channel` and
  `futures-channel` carry messages and keep no current value; `async-lock` has no watch.
- **The shape:** `Watch<T>` in `crates/ulo-rpc/src/watch.rs`: `modify` changes the value under
  the lock, releases it and calls `Event::notify(usize::MAX)`; `wait_for` registers a listener
  before reading the value and awaits it only if the predicate fails, so a change between the read
  and the wait wakes it. `read` replaces `borrow`.
- **Why `event-listener`:** it is the primitive `async-lock` and `async-channel` are built on, it
  needs no runtime, and it is already in the tree through `async-lock`, so the lock gains no
  package. The brief's list named `async-channel`, `async-lock`, `futures-channel` and the
  `futures` primitives; none of them is a watch. See S6.
- **A dropped sender:** tokio's `wait_for` answered `Err` once the sender was gone, which both
  sites ignored; nothing here can drop the value while a waiter borrows it.

### 3. The server takes its runtime from the app and refuses an app with none in `prepare`

- **Where it comes from:** `Mounted::app().runtime()`, stage a's `AppHandle::runtime()`, cloned
  into `Shared::runtime` and handed to `serve`'s `TaskSet`.
- **The refusal:** `prepare` pushes a failure when the app has no runtime, beside every other
  `prepare` failure, so `listen()` answers `StartupError::Configure` before any socket:
  "transport `Rpc`: the RPC server is bound on an app with no runtime, which it spawns every call
  on; set one with `.runtime(..)`, which sets the app's timer too". The last clause is for the
  common case, an app given `.timer(..)` alone.
- **Not in `listen()` beside `TimerMissing`:** the core does not know which transports spawn.
  HTTP spawns nothing since stage b, so a core refusal would need a flag on `Server` or a refusal
  of every app without a runtime. See S1.
- **After the refusal:** `prepare` builds no route table when the runtime is missing, which
  `into_result` has already reported; a `let`-`else` returns there with a comment saying why.
- **Every RPC app in the tree** moved from `.timer(ulo_tokio::Timer)` to
  `.runtime(ulo_tokio::Tokio::current())`: the conformance crate's server and client apps, the TCP
  and UDP `client_close`, `drain_end` and `deadline_answers` tests, and `ulo-rpc`'s `drain_window`
  and `stream_end`. UDP's `close_read` binds no RPC server and keeps its timer. The assertions of
  `drain_window` and `stream_end` are unchanged; each test's builder line is the one change.

### 4. `RpcClient`: given its runtime, never finding one

- **Inside an app:** `RpcClientModule` builds the client from `Dep<dyn Runtime>` in place of
  `Dep<dyn Timer>`, so an app with `.timer(..)` alone fails `wire()` naming the missing
  `dyn Runtime` binding; the module doc says so. The runtime's `Timer` half times every call.
- **Outside an app:** there was no public constructor; the only one, `RpcClient::new(Arc<L>,
  Bound, Arc<dyn Timer>)`, was `pub(crate)` and called by the module. `RpcClient::new(link,
  runtime)` is the public form: it takes the link by value and the runtime as `Arc<dyn Runtime>`,
  the type `AppHandle::runtime()` answers. The timeout is the module's
  default, five seconds, with `.timeout(d)` per call. See S2.
- **What the client spawns:** the reply router per connection, a streamed request's pump, and a
  dropped call's `cancel`. A client cannot route a reply without spawning, so every constructor
  takes a runtime and no client holds none. See S3.
- **In a drop:** a client built by `new` owns its link. `ClientInner`'s `Drop` creates the link's
  `close` future and spawns it on the client's runtime, wrapped with an `Unfinished` guard whose
  drop warns when the task is dropped before the close finished: a tokio runtime that has shut
  down drops a task it is handed at once, on the spawning thread (`OwnedTasks::bind_inner` shuts
  the task down once the list is closed, `tokio-1.53.2/src/runtime/task/list.rs:139-143`). The connection is released by the client's own fields
  dropping, whatever the runtime does with the task. A module's client closes nothing on drop;
  the module's `on_destroy` hook closes its link, as before.
- **A call keeps its client:** `Exchange` holds a clone of the client, so a client dropped while a
  call or reply stream it made is under way closes the link only once that call ends. A module's
  client is unaffected.

### 5. F360: `Tokio` holds the `Handle` it spawns on, and the unit form is gone

- **The form:** `Tokio { handle: Handle }`, built by `Tokio::current()`, which calls
  `Handle::current()` and panics there outside a runtime, or `Tokio::from_handle(h)`. `Spawn` is
  `self.handle.spawn(task)`; `Timer::sleep` and `now` enter the handle, so a sleep is registered
  with that runtime's timer and `now` reads its clock, a paused test clock included, from any
  thread.
- **The unit form does not stay:** it spawned on whatever runtime was current, which answer 5
  rules out for anything given to the app or a client, and which is F360's panic in a drop off a
  worker thread. Keeping it beside the handle form would keep both. `.runtime(Tokio::current())`
  is the one change at each call site. See S5.
- **What a held runtime cannot do:** once it has shut down, a task spawned on it is dropped unrun
  and its handle answers `Aborted`; the type's doc says the runtime must outlive what is spawned
  through it.
- **The conformance harness:** `runtime_suite!` called `Harness::runtime()` before `block_on`,
  outside any runtime; it now calls it inside the future it hands `block_on`. The harness trait is
  unchanged.
- **Left ambient:** `ulo_tokio::Timer` (`tokio::time::sleep` on the current runtime), `spawn` and
  `spawn_in`. Each is a free function or the clock alone, not the runtime given to an app or a
  client.

### 6. The `Link` SPI and the seven links

- **No signature changes:** `Link`, `Inbound`, `Outbound`, `Delivery`, `ReplyPath`, `Ack`,
  `Capabilities` and the error types name `BoxStream`, `BoxFuture`, `std` and `ulo` types alone.
  The one tokio type crossing the crate's boundary was `Body::Stream`'s receiver, which is
  crate-private.
- **The links stay tokio-based inside** (answer 2). Their `tokio::spawn` calls (TCP 4, UDP 1,
  NATS 4, Redis 3, RabbitMQ 5, MQTT 4, Kafka 5) each start a lane, router, pump or writer from
  `listen` or `connect`. None is a mechanical swap: a link's client side is never handed the app,
  and its server side sees it only as `&AppHandle` in `prepare`, where the runtime is an `Option`
  the link would have to store and fall back from. Their `select!`s and channels are link-internal.
- **Kafka's `spawn_blocking`** (`anchor` and `control_consumer`) stays a link-internal tokio call,
  as batch 18's S10 left it.
- **Kafka's backoff stays on `std::thread::sleep`:** `anchor` runs every broker call, the retried
  `committed_offsets` included, inside one `spawn_blocking` closure, so it cannot await the app's
  `Timer` between attempts. Awaiting it would take one `spawn_blocking` per attempt with the
  consumer shared across them. `coordinator_retried`'s doc said a link is not handed the timer,
  which stage a made false; it now gives the blocking thread as the reason.

## The tests

- **`crates/ulo-tokio/tests/handle.rs`, two tests,** each on a runtime the test builds and holds by
  `Tokio::from_handle`, with the work done on a plain `std::thread` that first asserts no runtime
  is current:
  - `spawns_from_a_thread_outside_any_runtime`: the spawned task runs, reported through a `std`
    channel within five seconds, and its handle answers `Finished`.
  - `sleeps_from_a_thread_outside_any_runtime`: a ten-millisecond sleep created on the thread
    ends when the runtime awaits it.
- **`crates/ulo-rpc/tests/runtime.rs`, three tests,** over a link whose client side connects at
  once and never replies, recording when the connection's `send` is dropped and when `close` runs,
  with drain_window's thread-tagged global `Capture` for the `warn`:
  - `the_server_refuses_an_app_with_no_runtime`: an app given `.timer(..)` alone binding an RPC
    server; `listen()` answers `StartupError::Configure` naming "no runtime" and
    "set one with `.runtime(..)`".
  - `a_dropped_client_closes_its_link_on_its_runtime`: a client from `RpcClient::new` on
    `Tokio::current()` connects through `emit`, is dropped, and its link's `close` runs within five
    seconds; the connection is released at the drop, and nothing is logged at `warn`.
  - `a_dropped_client_whose_runtime_has_shut_down_releases_its_connection_and_warns`: the client
    connects on a current-thread runtime, the runtime is dropped, then the client: the connection
    is released at the drop, the link's `close` never runs, and the `warn` naming the unfinished
    close is logged on the dropping thread. This stands for the brief's "a client dropped with
    none": no client holds none (decision 4, S3).
- **Changed:** `ulo-tokio`'s `tests/app.rs` and `tests/conformance.rs` build `Tokio::current()`,
  `overriding_the_runtime_is_refused` becoming a `#[tokio::test]` to have a runtime to capture;
  `ulo-rpc`'s unit test builds its client with `of_module` on `Tokio::current()` and loses its
  `NeverExpires` timer.
- **No RPC example exists in the workspace:** `examples/` is excluded from it, and its RPC files
  (`rpc_*.rs`, `error_telemetry.rs`) are written against the pre-redesign `ulo::rpc` API; none was
  run.

### Before and after

Each break rewrote one span through a script that asserted the span occurred once, ran the named
target, and wrote the file back byte for byte, checked by hash; the full output of every run is in
the scratchpad (`runtime-c/broken/`). Every run reported its tests by name.

| Break | Target | Result |
| --- | --- | --- |
| `Tokio::spawn` through `tokio::spawn`, the old form | `ulo-tokio --test handle` | `spawns_from_a_thread_outside_any_runtime` failed, the thread panicking "there is no reactor running, must be called from the context of a Tokio 1.x runtime"; the sleep test passed |
| `Tokio::sleep` without entering the handle | same | `sleeps_from_a_thread_outside_any_runtime` failed with the same panic; the spawn test passed |
| the dropped client drops its close and guard in place, spawning nothing | `ulo-rpc --test runtime` | `a_dropped_client_closes_its_link_on_its_runtime` failed after five seconds, "the dropped client did not close its link on its runtime"; the other two passed |
| the `Unfinished` guard's `warn` disabled | same | `a_dropped_client_whose_runtime_has_shut_down_..` failed, "no `warn` that the link's close did not run: []"; the other two passed |
| `prepare`'s runtime check disabled | same | `the_server_refuses_an_app_with_no_runtime` failed, "an RPC server on an app with a timer and no runtime was not refused in `prepare`"; the other two passed |
| the restored tree | `ulo-tokio`, `ulo-rpc` | every target passed |

Before the RPC test apps moved to `.runtime(..)`, `drain_window`'s three tests failed at
`listen()` with the refusal of decision 3 (`runs/drain_window-timer-only-refused.txt`), which is
the same refusal reached through the suites' own apps.

## Left for the transports DESIGN fold

- §0, principle 5 (line 13): "Transports may depend on tokio" no longer holds for `fw-rpc`, which
  follows the core's rule (answer 4); its links do depend on tokio.
- §1's crate table: `fw-rpc`'s row (line 34) gains "no runtime: tasks on the app's `Runtime`";
  `fw-tokio`'s row (line 43) names `Tokio` holding a `Handle`, built by `current()` or
  `from_handle(h)`.
- §5.3's server paragraph (line 946): `prepare` also refuses an app with no runtime, as a
  `Configure` error naming `.runtime(..)`; `serve` spawns each call on the app's runtime into a
  `TaskSet`.
- §5.4's client paragraph (line 987): "the client is a singleton reading `Dep<dyn Timer>`, so an
  application without a `Timer` fails `wire()`" reads `Dep<dyn Runtime>` and an application without
  a runtime; "A timeout uses the app's `Timer`" uses the client's runtime; "Dropping a stream or a
  pending request sends `cancel`, from a spawned task, and not at all outside a tokio runtime" is
  spawned on the client's runtime, from any thread. The paragraph gains `RpcClient::new(link,
  runtime)` for a client outside an app: it owns its link, a call holds the client, and its drop
  spawns the link's `close` on its runtime, a task the runtime drops unrun being logged at `warn`.
  F363 limits where such a client may be called from.
- §8 (line 1139): "`fw-tokio` provides three things" gains `Tokio`, the core's `Runtime` on a held
  `Handle`, spawning and timing on that runtime from any thread; `Timer`, `spawn` and `spawn_in`
  stay on the current runtime.
- §11's SPI table (line 1178): `RpcClient::new` and `Tokio::{current, from_handle}`.
- Stage a's log names §8 and §1's `fw-tokio` row for the unit `Tokio`; both read the handle form
  instead.

## Needs sign-off

### S1. The no-runtime refusal is the RPC server's, in `prepare`

Decision 3: an app binding an RPC server with no runtime fails `listen()` with
`StartupError::Configure`, the text naming `.runtime(..)`, collected beside every other `prepare`
failure. The alternative is a core `RuntimeMissing` beside `TimerMissing`, refused in `listen()`
before any `prepare`, which needs the core to know which servers spawn: a `Server` constant or
method, which stage d's `ulo-ws` would set too.

### S2. `RpcClient::new(link, runtime)`, the one public constructor

Decision 4: the link by value, the runtime as `Arc<dyn Runtime>`, the five-second default with no
client-wide timeout setting, `.timeout(d)` per call. The alternatives are `impl Runtime` for the
runtime, which an `Arc` from `AppHandle::runtime()` does not satisfy, and a `timeout: Bound`
argument or a builder.

### S3. No client holds no runtime

Decision 4: a client spawns its reply router, so every constructor takes a runtime, and answer 5's
"if it was never given one" has no state to describe. The branch that remains is a runtime that
drops the close task unrun, one that has shut down: the connection is released by the drop itself
and the unfinished close is logged at `warn`. The brief's test of a runtime-less drop is that case.
The alternative is an optional runtime, with a client given none unable to receive a reply.

### S4. A client from `new` closes its link when dropped, and a call holds its client

Decision 4: the last clone and the last call ending spawn the link's `close`; a module's client
leaves its link to the module's `on_destroy`. The alternatives are a client whose link the caller
closes explicitly, an `RpcClient::close(&self)`, with the drop warning when it was not called, or a
dropped client closing the link with calls still under way.

### S5. `Tokio` holds a `Handle`, and the unit form is removed

Decision 5 and F360: one form, spawning and timing on the runtime it was built on, from any thread.
`.runtime(ulo_tokio::Tokio)` no longer compiles and is written `.runtime(ulo_tokio::Tokio::current())`.
The alternatives are keeping the unit form beside it for apps that only spawn from inside their
runtime, or a second type for the handle form. The chat has not answered F360; this is its
proposed fix as built.

### S6. The watch is `event-listener` over a `std` mutex

Decision 2: no crate in the brief's list is a watch; `event-listener` is the primitive under
`async-lock` and `async-channel` and adds no package to the lock. The alternatives are
`async-lock`'s `Mutex` plus an `async-channel` used as a change notification, or a watch type in
`ulo-transport`, which stage d's `ulo-ws` could share.

### S7. `RpcClientModule` reads `Dep<dyn Runtime>`

Decision 4, as the brief stated it: an app with `.timer(..)` alone and an RPC client fails
`wire()`. The alternative is a module that also accepts a timer alone and spawns nothing, which
cannot route a reply.

### S8. The links keep `tokio::spawn`, and Kafka's backoff its blocking sleep

Decision 6: none of the 26 spawns is a mechanical swap, a link's client side being handed no app;
Kafka's `spawn_blocking` stays, and its backoff stays on `std::thread::sleep` inside it. The
alternatives are a runtime parameter on `Link::connect`, which changes the SPI, and an `anchor`
that awaits the app's `Timer` between per-attempt `spawn_blocking` calls.

## Verification

Full, unfiltered output of every run is in the session scratchpad, `runtime-c/`: `tree/`,
`suites/` (with `summary.txt`, each run's exit code and container count), `verify/`, `broken/`
and `probe/`.

- **The tree check,** first against violations. On the tree before this stage, `cargo tree -p
  ulo-rpc -e normal --all-features -i tokio` printed `tokio v1.53.2` / `└── ulo-rpc`
  (`tree/before-stdout.txt`). On the changed tree, with `tokio = { workspace = true, features =
  ["sync"] }` appended to `ulo-rpc`'s dependencies, it printed the same (`tree/violation.txt`); the
  manifest and `Cargo.lock` were restored from copies, checked by hash. On the final tree it prints
  nothing on stdout and exits 0 with "nothing to print" on stderr, tokio being a dev-dependency of
  `ulo-rpc`, and the same with `--target all` (`tree/final-*.txt`). `cargo tree -p ulo-rpc -e
  normal --all-features` names no `tokio`, `async-std` or `smol`.
- **Conformance,** three consecutive runs per suite on the final tree, one suite at a time, test
  time as libtest reports it; the brokers with `--features integration --test conformance
  --locked`:

| Suite | Run 1 | Run 2 | Run 3 | Result |
| --- | --- | --- | --- | --- |
| `ulo-rpc-tcp` conformance | 1.31 s | 1.31 s | 1.31 s | 25 passed |
| `ulo-rpc-tcp` conformance_cbor | 1.31 s | 1.31 s | 1.31 s | 25 passed |
| `ulo-rpc-tcp` conformance_tls | 1.32 s | 1.31 s | 1.31 s | 25 passed |
| `ulo-rpc-udp` conformance | 1.31 s | 1.30 s | 1.31 s | 24 passed, 1 ignored |
| `ulo-rpc-nats` | 10.47 s | 7.17 s | 9.01 s | 25 passed |
| `ulo-rpc-redis` | 7.21 s | 7.02 s | 7.25 s | 26 passed: the suite's 25 and the link test |
| `ulo-rpc-mqtt` | 15.87 s | 12.71 s | 8.66 s | 30 passed: the suite's 25 and five link tests |
| `ulo-rpc-kafka` | 38.67 s | 34.68 s | 35.65 s | 25, 24 and 25 passed; see below |
| `ulo-rpc-kafka`, again | 29.47 s | 29.84 s | 27.19 s | 25 passed |
| `ulo-rpc-rabbitmq`, `ULO_CONFORMANCE_PARALLEL=6` | 62.62 s | 58.79 s | 77.16 s | 25 passed |

  Kafka's second run failed one scenario before it ran: `domain_error_envelope`'s Kafka container
  did not start, its log ending at "===> Launching ..." before the ready line testcontainers waits
  for. No ulo code had run. Three more runs passed every scenario.
- **The other link tests,** `cargo test -p <crate> --locked`, once each: `ulo-rpc-tcp`'s
  `client_close` (1), `deadline_answers` (2), `drain_end` (1) and its three suites; `ulo-rpc-udp`'s
  `client_close` (1), `close_read` (1), `drain_end` (1) and its suite; `ulo-rpc-kafka`'s unit tests
  (3); `ulo-rpc-nats`, `-redis`, `-rabbitmq` and `-mqtt` compile their suites without the feature
  and run nothing.
- `ulo-rpc`: `drain_window` 3, `stream_end` 4, `runtime` 3, the unit test 1, five doc examples
  ignored. `ulo-tokio`: `app` 5, `conformance` 12, `handle` 2. `ulo-codegen-tests` ran in the
  workspace pass, every target passing.
- **Each RPC crate compiled alone,** `cargo check -p <crate> --all-targets --locked` for
  `ulo-rpc`, the seven links, `ulo-rpc-conformance`, `ulo-tokio` and `ulo-runtime-conformance`:
  each exit 0, so no link compiled only through a tokio feature `ulo-rpc` used to enable.
  `cargo check -p ulo-rpc --lib --locked` built no tokio.
- `cargo check --workspace --all-targets --all-features` with
  `CFLAGS=-I/opt/homebrew/opt/openssl/include LDFLAGS=-L/opt/homebrew/opt/openssl/lib`, and
  without `--all-features`: exit 0, the 17 known warnings in `crates/ulo/src` and no warning
  location elsewhere.
- `cargo +1.88 check --workspace --all-targets --exclude ulo-http-salvo --exclude
  ulo-graphql-async-graphql`: exit 0, the same 17 and no other.
- `cargo test --workspace --no-fail-fast` with the OpenSSL flags: 501 passed, 0 failed, 66 ignored
  across 131 test binaries: stage b's 496, the two `handle` tests and the three `runtime` tests;
  the ignored one added is `RpcClient::new`'s `ignore` example.
- `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --lib -p <crate>` for `ulo-rpc`, `ulo-tokio`,
  `ulo-runtime-conformance`, `ulo-rpc-conformance`, `ulo-rpc-kafka`, `ulo-rpc-tcp` and
  `ulo-rpc-udp`: each passes.
- `cargo +1.98.1 clippy` over those seven crates with `--all-targets --no-deps --locked`: exit 0,
  29 warning locations, none on a line this stage changed or in a new file, by a checker reading
  `git diff -U0`'s hunks and the untracked files; run first against four planted locations, two on
  changed or new lines and two not, it reported the two (`verify/checker-selftest.txt`). The
  macros' output did not change; `cargo +1.98.1 clippy -p ulo-macro-lints --all-targets --no-deps
  --locked -- -D warnings` exited 0 all the same.
- **Containers:** the baseline is the user's four, seaweedfs, mailpit, postgres:18 and redis:7,
  untouched throughout. Every container the broker suites started was removed by the test that
  started it. A fifth, `mqttprobe-a632688a` (`eclipse-mosquitto:2.0.18`, host port 18839), appeared
  during Redis's first run and was gone before the workspace test; it belongs to another task in
  this session working in the shared scratchpad's `mqtt-probe/`, was not started here, and was not
  touched. The counts in `suites/summary.txt` include it from Redis's first run to the MSRV check.
