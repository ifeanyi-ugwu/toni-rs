# Divergences: runtime neutrality, stage e1: each tokio link and `ulo-ws-redis` hold their own runtime (F363, S8, F367), `Tokio::run`, the conformance scenario calling a client from a plain thread, a two-process Redis broadcast test, and `ulo-smol` with the runtime suite on it

The thirty-sixth response puts F363's fix inside the link: each tokio-based link captures a tokio
handle when it is built, with an explicit `with_handle(h)` form, runs its I/O there and hands the
results back, and its `tokio::spawn` calls spawn on that handle (S8). This stage builds that on
the seven links through one helper, `ulo_tokio::Tokio::run`, so a link's futures and streams no
longer depend on the executor polling them. The RPC conformance suite gains a scenario that calls
`RpcClient::new` from a plain `std::thread` through `futures::executor::block_on`; it passes on all
seven links, and fails on TCP and NATS with their link restored to `6f9d6779`, where stage c's probe
failed. `ulo-smol` is the new crate beside `ulo-tokio`: `Smol` implements `Timer` and `Spawn` over
an `async-executor` `Executor` the caller owns and `async-io`'s timers, and passes the runtime suite's
twelve scenarios on a four-thread executor; with `abort` as a plain drop of the `Task`, two of them
fail. The `Link` trait and the hubs are unchanged. F367 joined the stage by a scope addition the
user signed off during the build, after the thirty-seventh response: `ulo-ws-redis` gets the links'
rule, and its first test against a real Redis, two processes with a member of one room each and a
room broadcast from either reaching the other.

Files changed: the workspace `Cargo.toml` (the member `crates/ulo-smol`; `async-executor`,
`async-io` and `futures-executor` in `[workspace.dependencies]`) and `Cargo.lock`; the new crate
`crates/ulo-smol` (`Cargo.toml`, `src/lib.rs`, `tests/conformance.rs`);
`crates/ulo-tokio/{Cargo.toml, src/lib.rs, tests/handle.rs}`; for each of the seven links,
`crates/ulo-rpc-<link>/{Cargo.toml, src/link.rs, tests/handle.rs (new)}`;
`crates/ulo-rpc-conformance/{Cargo.toml, src/lib.rs, src/cases/mod.rs, src/cases/threads.rs (new)}`;
`crates/ulo-rpc/src/link.rs` (module doc only); `crates/ulo-ws-redis/{Cargo.toml, src/lib.rs,
tests/handle.rs (new), tests/integration.rs (new)}`; `.github/workflows/ci.yml` (one step in the
broker job); in the workspace's `FRAMEWORK_GAPS.md`, notes under F363 and F367, and F369 and F370
filed. The tree is `6f9d6779` plus this stage.

## The signatures

```rust
// ulo_tokio, additions
impl Tokio {
    pub fn try_current() -> Option<Tokio>;   // `None` outside a tokio runtime
    pub fn handle(&self) -> &tokio::runtime::Handle;
    pub fn run<F>(&self, fut: F) -> impl Future<Output = Result<F::Output, Stopped>> + Send + 'static
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stopped;                          // Display, std::error::Error

// each of ulo_rpc_tcp::Tcp, ulo_rpc_udp::Udp, ulo_rpc_nats::Nats, ulo_rpc_redis::Redis,
// ulo_rpc_mqtt::Mqtt, ulo_rpc_rabbitmq::RabbitMq, ulo_rpc_kafka::Kafka, and ulo_ws_redis::Redis
pub fn with_handle(self, handle: tokio::runtime::Handle) -> Self;

// ulo_smol, new crate
pub use async_executor::Executor;
#[derive(Clone)]                             // Debug by hand, naming no field
pub struct Smol { /* private: Arc<Executor<'static>> */ }
impl Smol {
    pub fn new(executor: Arc<Executor<'static>>) -> Smol;
    pub fn executor(&self) -> &Arc<Executor<'static>>;
}
impl ulo::Timer for Smol {}                  // `async_io::Timer::after`, `Instant::now()`
impl ulo::Spawn for Smol {}                  // `Executor::spawn` through `TaskHandle::launch`

// ulo_rpc_conformance, a scenario, stamped as `client_on_a_thread_without_a_runtime`
pub mod cases { pub mod threads { pub async fn plain_thread<B: Broker>(); } }
```

Unchanged: `Link`, `Inbound`, `Outbound`, `Delivery`, `ReplyPath`, `Ack`, `Capabilities`,
`RpcClient`, `Server`, `Runtime`, `Spawn`, `RuntimeTask`, `TaskHandle`, `Harness`,
`runtime_suite!` and `BroadcastAdapter`. No handoff needed a signature change: a link's futures already answer through
`BoxFuture` and `BoxStream`, and what crosses back is a value or a channel's receiver.

Private: the sides that spawn or publish hold the link's `Tokio`, the server and client sides of
Redis, RabbitMQ and Kafka, MQTT's two and NATS's client side; TCP's `Server` holds the
`Handle` its connections spawn on and `forward` and `refuse` take it; UDP's `receive` and
`reply_path` take it; NATS's `open` and `open_with` take the URL by value; RabbitMQ's
`State::client` is `(Connection, Tokio)` and its `ServerSide::runtime` a `Tokio` where it was the
`Handle::current()` of `listen`; TCP and UDP gain a `read_replies` task each.

Dependencies: the seven links gain `ulo-tokio` as a normal dependency (it was a dev-dependency of
TCP and UDP) and `futures-executor` as a dev-dependency; `ulo-rpc-conformance` and `ulo-tokio`'s
tests gain `futures-executor`; `ulo-smol` depends on `ulo`, `async-executor` 1.14 and `async-io` 2.6,
and on `ulo-runtime-conformance`, `async-io` and `futures` for its tests. `ulo-ws-redis` gains
`ulo-tokio`, an `integration` feature, and dev-dependencies on `ulo-rpc-conformance` (its container
helpers), `ulo-ws-hyper`, `futures-executor`, `futures-util` with `sink`, `testcontainers`, `tokio`
and `tokio-tungstenite` with `connect`. `Cargo.lock` gains
`async-io` 2.6.0, `polling` 3.11.0 and `ulo-smol`; `async-executor` 1.14.0, `async-task` 4.7.1 and
`futures-executor` 0.3.34 were already in it.

## Decisions

### 1. The rule, the same on all seven links

- **Capture.** A link's constructor (`Tcp::new`, `Udp::new`, `Nats::url`, `Redis::url`,
  `Mqtt::url`, `RabbitMq::url`, `Kafka::brokers`) takes `Tokio::try_current()`, the runtime current
  where it is built, or none. `with_handle(h)` replaces it at any point before use. Nothing looks
  for a runtime later.
- **No runtime, refused at first use.** A link holding none refuses in `prepare`, so a server's
  `listen()` answers `StartupError::Configure` before any socket, and in `connect`, so a client's
  call fails `Unavailable`, "the {link} link could not connect: …". Both name `.with_handle(..)`:
  "the TCP link has no tokio runtime: build it inside one, or give it one with `.with_handle(..)`".
  TCP collects it with its other `prepare` problems; the others return it first.
- **Why not panic at construction, as `Tokio::current()` does:** `Tcp::new(addr).with_handle(h)`
  on a smol thread would panic in `new` before `with_handle` could run. A panicking constructor
  needs a second constructor per link that takes the handle, and the brokers' builders take one
  argument each today. See S1.
- **Tasks.** Every task a link starts spawns on its handle by name, `handle.spawn(..)` or
  `JoinSet::spawn_on(.., handle)`; no link calls `tokio::spawn`. `tokio::time` is used only inside
  those tasks, which run on the link's runtime.
- **Futures.** What needs tokio's context, opening a socket, a connection or a client (which start
  the library's own tasks on the current runtime), or a library future that uses tokio's timer or
  spawner when polled, runs on the link's runtime through `Tokio::run`. A library call whose future
  only hands a command to the library's own task, already on the link's runtime, over a channel
  that needs no runtime, stays where it is polled: rumqttc's `AsyncClient` (a flume request
  channel, read in `rumqttc-0.25.1/src/v5/client.rs`), async-nats's `Client::publish`, `subscribe`,
  `flush` and `drain` (a tokio `mpsc` command channel, `async-nats-0.46.0/src/client.rs:447-748`;
  its timeouts are in `request`, which the link does not call), and TCP's writes, which go over the
  link's own `tokio::sync::mpsc` queue to its writer task. Redis runs through `run`:
  `ConnectionManager` answers within a 500 ms response timeout by default
  (`redis-1.7.1/src/client.rs:180`), which is `tokio::time::timeout`. Kafka does: `FutureProducer::send`
  waits on tokio's clock when librdkafka's queue is full (`rdkafka-0.39` `src/producer/future_producer.rs:334`).
  RabbitMQ does: lapin 4 takes tokio's current runtime when it connects (`lapin-4.12.0/src/connection.rs:334`,
  `src/runtime.rs:29`), and its other futures were not read. UDP's socket sends are I/O and run through `run`. See S4.
- **Streams.** Every stream a link hands out reads a channel that needs no runtime, fed by a task on
  the link's runtime. TCP's and UDP's client reply lanes read their socket in the caller's poll;
  each now has a `read_replies` task on the link's runtime feeding a `tokio::sync::mpsc` of 64
  frames, which ends at the link's `close`, at the connection's end, or once the lane is dropped
  (`Sender::closed`). The other streams were already channel receivers.

### 2. Per link

| Link | Through `run` | Spawned on the handle (the 26 `tokio::spawn`s of stage c, recounted at `6f9d6779`) | Inline |
| --- | --- | --- | --- |
| TCP | `connect`'s `TcpStream::connect` | the accept loop, `forward`, each connection's writer, `refuse`'s answer (4); each accepted connection and each client writer through `JoinSet::spawn_on`; the client reply lane's `read_replies` (new) | `listen`'s `TcpListener::from_std` under `handle.enter()`, a synchronous registration; `send` and the reply path, pushes onto the write queue; `drain` and `close`, watches and `JoinHandle`s |
| UDP | `listen`'s and `connect`'s binds and `connect`; `send`; the reply path's `send_to`; `close`'s `final_read` | `receive` (1); the client reply lane's `read_replies` (new) | `drain`, a watch |
| NATS | `listen`'s and `connect`'s `ConnectOptions::connect` | both request lanes and the control lane, the reply router, a streamed request's pump (4) | subscriptions, flushes, publishes and `shut`, command handoffs |
| Redis | `listen`'s and `connect`'s `ConnectionManager` and Pub/Sub subscription; `send`; the reply path; `drain`'s `unsubscribe` | the server and client lanes, the drain's watcher (3) | `close`, aborts |
| MQTT | none | both event loops, the drain's watcher, a streamed request's pump (4) | publishes, unsubscribes, the disconnect, request-channel handoffs |
| RabbitMQ | `listen`, `connect`, `send`, the reply path, `drain`'s `basic_cancel`, `close` | both lanes, the reply router, the drain's watcher, a streamed request's pump (5); an `Ack`'s settlement, which spawned on `listen`'s `Handle::current()` | none |
| Kafka | `listen` (topic creation, the anchor, the consumers, the producer), `connect`, `send`, the reply path | both lanes, the reply router, the drain's watcher, a streamed request's pump (5) | `drain`'s pause and asynchronous commit, and `close`'s synchronous commit, which blocks the thread polling it (F370) |

- **Kafka's own threads** (batch 19's `blocking` and `Detached`) are unchanged. `anchor` still waits
  on the app's `Timer` between retries, inside the `run` task, so a smol app's timer is awaited on a
  tokio worker; `async-io`'s timers fire whichever executor waits on them.
- **Where the setup's results are kept.** `listen` and `connect` run only their I/O through `run`,
  which answers the connection, the subscriptions and the consumers; the side is built, its tasks
  spawned and the link's state written after `run` answers, in the caller's poll, with no await in
  between. A `listen` or `connect` dropped while `run` is under way aborts the task and drops what it
  opened. MQTT's event loop is spawned before the broker answers and is the exception, F369.

### 3. `Tokio::run`

- **Shape:** spawned on the held `Handle` at the answer's first poll, an `async move` block holding
  the future until then, so answers created in order and awaited in order run in order. The answer
  holds the task's `JoinHandle` in an `Aborting` wrapper that aborts on drop, so a dropped `send` or
  `connect` stops its work as the inline future's drop did.
- **The handoff is the `JoinHandle`.** tokio polls a `JoinHandle` with no runtime context: its
  `poll` reads the coop budget, which outside a runtime is unconstrained
  (`tokio-1.53.2/src/runtime/task/join.rs`, `impl Future for JoinHandle`), and stage a's
  `ulo_tokio::Tokio` already hands one to whatever polls its `TaskHandle`. The brief's
  "runtime-neutral channels" is met by it rather than by a separate oneshot. See S3.
- **A panic** in the task resumes where the answer is awaited, as it did when the future was polled
  inline. **A runtime that has shut down** drops the task unrun and the answer is
  `Err(Stopped)`, which each link passes on as a `BoxError` with `?`, or ignores in `drain`, which
  answers nothing.
- **In `ulo-tokio`, public.** The links reach it through the dependency they gain, and the link
  side's `Tokio` is the same type the app and a client are given. See S2.

### 4. MQTT's and NATS's handoffs stay inline: a first build ran them through `run`

The first build ran every library future of every broker link through `run`. MQTT's suite then
failed `a_drain_dropped_before_the_broker_confirms_logs_the_filters` in each of its three runs
(`suites/mqtt-{1,2,3}.txt`): the test polls the drain once on a current-thread runtime and drops
it, and with the UNSUBSCRIBEs queued from a task, the first poll only spawned that task and the
drop aborted it before it ran, so no UNSUBSCRIBE was queued and the `Unconfirmed` guard that logs
the filters never existed. rumqttc's `AsyncClient` methods are request-channel sends to the event
loop, already on the link's runtime, so MQTT's drain, sends, reply path and close returned to
inline, which restores the UNSUBSCRIBEs queued at the drain's first poll; async-nats's client calls
are command handoffs of the same kind, and NATS was brought to the same rule. Both suites then
passed three runs each (`suites/{nats,mqtt}-r2-*.txt`). The rule in decision 1 is the one built.

### 5. The F363 test is a conformance scenario, written once

- **Where:** `crates/ulo-rpc-conformance/src/cases/threads.rs`, stamped per link by
  `conformance_suite!` as `client_on_a_thread_without_a_runtime`, so each link's suite carries it:
  TCP's three, UDP's, and the brokers' under `integration`. A test file per link crate would repeat
  the server, the handlers and each broker's container start.
- **What it does:** starts the usual fixture, builds the client's link through
  `Broker::client_link` on the scenario's runtime, which the link captures, and moves it to a plain
  `std::thread` that asserts no tokio runtime is current, builds `RpcClient::new(link,
  Arc::new(Tokio::current()))` and runs through `futures_executor::block_on`: a unary call, an event
  (checked at the server's probe afterwards), and, where the link carries the streamed shapes, a
  server stream and a client stream, whose pump exercises the links' streamed-request spawns. The
  client is dropped on the thread. The scenario joins the thread through `spawn_blocking` within 60 s
  and reports a panic's message. The client's runtime is the scenario's tokio runtime: a smol client
  runtime would put `ulo-smol` under the RPC suite, which the stage leaves for the suites taking a
  runtime.
- **The other paths of the rule** are per crate, in `tests/handle.rs`: on all seven, a link built on
  the test's thread, which has no runtime, refuses `connect` naming `.with_handle(..)`; on TCP also
  the server refused at `listen()` as `Configure`, and a link built on a plain thread with
  `with_handle` serving a client called from that thread. See S6.

### 6. `Smol`

- **Constructed from an `Arc<Executor<'static>>` the caller owns and runs**, as `Tokio` holds a
  `Handle`. smol's global executor lives in the `smol` crate, which would bring `async-net`,
  `async-fs`, `async-process` and `blocking`, and starts its own threads sized by `SMOL_THREADS`; a
  runtime value naming its executor leaves the threads to the caller and lets a test drive it. A task
  spawned on an executor nothing runs again is never polled; the doc says so. See S7.
- **`abort` cancels through `Task::cancel`.** `Task`'s drop marks it cancelled and returns at once,
  with the future still being polled on another thread or not yet dropped by the executor; stage a's
  S1 requires `poll_ended` to answer only once the future is dropped. `abort` takes the
  `FallibleTask`, calls `cancel()`, which is an `async fn` marking the task closed on its first poll,
  polls it once with `Waker::noop()` so the abort takes effect whether or not the handle is polled
  again, and keeps it; `poll_ended` polls it to its end, which `async-task` answers only once the task
  is neither scheduled nor running (`async-task-4.7.1/src/task.rs:327-348`). `detach` detaches a
  running task and drops a cancelling one, whose `Task` drop only re-marks it cancelled.
- **`FallibleTask`**, not `Task`: a task its executor dropped unrun answers `None` rather than
  panicking with "Task polled after completion", and the handle reads `Aborted`.
- **Panics** reach `TaskHandle::launch`'s catch inside the task, so the task returns and
  `async-executor`'s own `propagate_panic(true)` sees no panic: `Panicked` is reported as on tokio.
- **The clock** is `async_io::Timer::after(d)`, which `async-io`'s reactor fires whatever executor
  awaits it, and `Instant::now()`, the clock it measures against. A duration past `Instant`'s range
  becomes `Timer::never()` inside `async-io`.
- **MSRV:** `async-executor` declares 1.65, `async-io` 1.71, `async-task` 1.57; the workspace's 1.88
  check builds the crate and its tests.

### 7. The harness drives a four-thread executor

`runtime_suite!` calls `Harness::runtime()` inside the future it hands `block_on`. The harness keeps
the scenario's executor in a thread-local that `block_on` sets on the test's thread before running
`async_io::block_on(executor.run(fut))`, and three more threads run the same executor until the
scenario returns, stopped through a shared `oneshot`, so a task may be polled on another thread than
the one aborting or awaiting it. A single-threaded harness would never reach the case decision 6 is
written against.

### 8. `ulo-ws-redis` under the same rule (F367)

- **Capture and tasks:** `Redis::url` takes `Tokio::try_current()`, `with_handle(h)` replaces it,
  the subscription task spawns on its handle, and its reconnect sleep runs inside that task.
- **`publish` runs through `Tokio::run`:** its connection is a `MultiplexedConnection`, whose driver
  task starts on the current runtime, and each `PUBLISH` answers within redis's response timeout on
  tokio's clock, as on the RPC link.
- **No runtime:** `BroadcastAdapter` has two methods and no step before first use, so the refusal
  reaches the first `publish` as its error, which `Broadcast::emit` turns into a `BroadcastError`,
  and the `subscribe` the hub calls when a server binds as a stream that ends at once, after a
  `tracing::error!` naming `.with_handle(..)`. The app then binds and serves with no broadcast
  reaching it, where a link's refusal fails `listen()`. See S9.

### 9. The two-process Redis test

- **Two processes:** `NodeId::current()` is one per process, and a client is addressed through its
  node's channel, so two apps in one process share a node and are not what the crate exists for.
  The second process is the test binary run again (`std::env::current_exe()`) as the ignored test
  `peer_instance`, with the Redis URL in `ULO_WS_REDIS_PEER`; it prints the address its server
  bound, which the parent reads off its stdout, and stops its app and exits when its stdin closes.
  A guard kills it if the test ends any other way. The ignored test does nothing with the variable
  unset.
- **The environment:** Redis 7 in a container per run, started through `ulo-rpc-conformance`'s
  `relay::unshadowed` and `reachable` as the broker suites start theirs; each instance serves the
  standalone server, `ulo_ws_hyper::Server`, on port 0, with a `port = own` gateway whose
  `OnConnect` joins the room and whose `shout` broadcasts to it.
- **Waiting for the subscriptions:** a broadcast published before the other process subscribes is
  lost, and Redis offers no count that shows the room's pattern subscription in place (`PUBSUB
  NUMPAT` counts distinct patterns, and both processes subscribe the same one). So the member on
  this process shouts every 200 ms, up to 60 s, until the member on the other receives one; that
  receipt is the positive signal, and the reverse direction is then one broadcast, this process's
  member reading past its own earlier shouts to it.
- **CI:** the broker job, which runs on push, gains `cargo test -p ulo-ws-redis --features
  integration --test integration --locked`; the workspace check's `--all-features` compiles it as
  it does the broker suites. The job keeps its name, `rpc conformance (brokers)`. See S10.

## The tests

- **`client_on_a_thread_without_a_runtime`**, on every link (decision 5). In the suites' runs:
  TCP's three suites 27 passed each, UDP 26 and 1 ignored, NATS 27, Redis 28, MQTT 32, RabbitMQ 27,
  Kafka 27.
- **`crates/ulo-tokio/tests/handle.rs`**, four added: `run_answers_on_a_thread_outside_any_runtime`
  (a `tokio::time::sleep` inside the run future, from a plain thread, answers `Ok(7)`);
  `dropping_the_answer_aborts_the_task` (an answer polled once and dropped drops the task's future,
  seen through a guard); `a_panic_in_the_task_resumes_where_the_answer_is_awaited`;
  `a_runtime_that_has_shut_down_answers_stopped`.
- **`crates/ulo-rpc-<link>/tests/handle.rs`**: `a_link_built_outside_a_runtime_and_given_none_refuses_to_connect`
  on all seven; on TCP also `a_server_link_built_outside_a_runtime_and_given_none_is_refused_in_prepare`
  and `a_link_given_a_handle_on_a_plain_thread_is_called_from_there`.
- **`crates/ulo-smol/tests/conformance.rs`**: the twelve scenarios of `runtime_suite!(OnSmol)`.
- **`crates/ulo-ws-redis/tests/handle.rs`**: `an_adapter_built_outside_a_runtime_and_given_none_refuses`,
  a publish failing naming `.with_handle(..)` and a subscription answering `None` at once.
- **`crates/ulo-ws-redis/tests/integration.rs`**, under `integration`:
  `a_room_broadcast_reaches_the_member_on_the_other_process` (decision 9), and `peer_instance`,
  ignored, its second process.

### Before and after

Each break went through `runtime-e1/scripts/brk.py`, which replaces each span (asserting it occurs
once) or restores a whole file to `HEAD`, runs the target, writes every file back byte for byte with
`Cargo.lock` among them, and compares a hash over `git diff HEAD` of `crates`, `Cargo.toml` and
`Cargo.lock` and every untracked file under `crates`; every restore reported `ok`, the hash
unchanged, but the first run of the planted tree check (below). Specs are in `runtime-e1/specs/`, full output in `runtime-e1/broken/`.

| Break | Target | Result |
| --- | --- | --- |
| `crates/ulo-rpc-tcp/src/link.rs` as at `6f9d6779` | the TCP scenario | failed, the thread panicking "there is no reactor running" at `tokio-1.53.2/src/net/tcp/stream.rs:164` (twice: on the first tree and on the final one) |
| `crates/ulo-rpc-nats/src/link.rs` as at `6f9d6779` | the NATS scenario, a NATS container | failed the same way, at `async-nats-0.46.0/src/connector.rs:176` (twice) |
| NATS's reply router spawned with `tokio::spawn` | the NATS scenario | failed, "there is no reactor running" |
| TCP's `connect` without `run` | the TCP scenario | failed, the panic at `stream.rs:164` |
| TCP's `read_replies` spawned with `tokio::spawn` | the TCP scenario | failed, the panic at `crates/ulo-rpc-tcp/src/link.rs:279` |
| TCP's `prepare` without the runtime check | TCP's `handle` tests | `..is_refused_in_prepare` failed: "transport `Rpc` failed to bind: the TCP link has no tokio runtime …", a bind failure where `Configure` was expected |
| `Aborting`'s drop not aborting | `ulo-tokio`'s `handle` | `dropping_the_answer_aborts_the_task` failed after its 5 s wait |
| `run` awaiting the future inline | same | `run_answers_on_a_thread_outside_any_runtime` failed, "there is no reactor running"; `a_runtime_that_has_shut_down_answers_stopped` failed, `Ok(7)` for `Err(Stopped)` |
| a panic answered as `Stopped` | same | `a_panic_in_the_task_resumes_where_the_answer_is_awaited` failed |
| `Smol`'s `abort` dropping the `Task` | `ulo-smol`'s suite | `abort_is_aborted_once_the_future_is_dropped` failed, "the handle answered before the task's future was dropped", left 0; `abort_all_aborts_every_task` failed, "futures dropped once every end was taken", 0 of 3 |
| `tokio` added to `ulo-smol`'s dependencies | `cargo tree -p ulo-smol -e normal -i tokio` | printed `tokio v1.53.2` / `└── ulo-smol` |
| `ulo-ws-redis`'s subscription task never started (its loop's condition `false`) | the two-process test, a Redis container | failed after 60 s: "none of 297 broadcasts from this process reached the member on the other within 60s"; the second process was killed by its guard |
| `crates/ulo-ws-redis/src/lib.rs` as at `6f9d6779` | `ulo-ws-redis`'s `handle` test | failed, the publish panicking "there is no reactor running" at `redis-1.7.1/src/aio/runtime.rs:157` |
| a link to a missing item in `ulo-smol`'s docs | `RUSTDOCFLAGS="-D warnings" cargo doc -p ulo-smol` | error, "unresolved link to `NoSuchItem`" |

The first run of the planted tree check restored `Cargo.toml` and not the lock, which the planted
`cargo tree` had rewritten; the next `cargo tree` rewrote it back, the hash then matching the one
before, and `brk.py` has saved and restored `Cargo.lock` on every run since, the check run again
under it (`broken/smol-tree-planted-2.txt`).

The S8 spawns inside a task already on the link's runtime (a streamed request's pump, TCP's
`refuse`, the lanes) would behave the same through `tokio::spawn`, the runtime current there being
the link's; no test tells the two apart. The ones in the caller's poll, the reply routers and
readers spawned by `connect`, are what the rows above break.

## Left for the transports DESIGN fold

Line numbers are those read on the working tree during this stage; another agent was editing the
file meanwhile.

- §0, principle 5 (line 13): a runtime enters through `fw-tokio` and, for an app, `fw-smol`; the
  seven links each hold a tokio runtime of their own (`Tokio`, captured where built or named by
  `with_handle`), so they work under an app on any runtime.
- §1's crate table: `fw-tokio`'s row (line 44) gains `try_current()`, `handle()` and `run(fut)`
  with `Stopped`; a row for `fw-smol` after it: `Smol`, the core's `Runtime` on an `async-executor`
  `Executor` the caller runs, `async-io`'s timers, no tokio in its tree; `fw-runtime-conformance`'s
  row (line 45) names `fw-smol` among the crates running it.
- §5.2, the RPC conformance paragraph (line 904) or the scenario list it governs: the scenario
  calling `RpcClient::new` from a plain thread through `futures::executor::block_on` (decision 5).
- §5.3, links: the rule of decision 1, the capture, the refusal in `prepare` and `connect`, the
  handle every task spawns on, what runs through `Tokio::run` and what stays a handoff; the link
  module's own doc states the requirement (`crates/ulo-rpc/src/link.rs`).
- §5.4's client paragraph (line 1021): its last sentence, "The link's own futures … are polled
  inside the caller's future, and a tokio-based link needs a tokio runtime current there, whatever
  runtime the client was given (F363)", no longer holds: the links run their I/O on their own
  runtime, and a client is called from any executor.
- §8, "`fw-tokio` provides four things" (line 1225): `Tokio` gains `try_current`, `handle` and
  `run`, the last spawning on first poll, aborting on drop, resuming a panic and answering
  `Stopped` once the runtime has shut down; a paragraph for `fw-smol` beside it (decisions 6, 7).
- §8's conformance paragraph (line 1232): `fw-smol` runs the suite too, on a four-thread executor.
- §8's table: `fw-smol` joins the "no tokio in the normal tree" row (line 1238); the links' row
  (line 1244) loses "a link's client side is handed no app, its `connect` and `send` polled in the
  caller's future (F363); 26 `tokio::spawn` calls …" for each link holding its own `Tokio` and
  spawning on its handle, `fw-ws-redis`'s clause unchanged (F367).
- Decision 46 (line 1460): "the links stay tokio-based behind the runtime-neutral `Link`" gains
  "each on a tokio runtime of its own, so an app on another runtime calls them".
- §4.3, the Redis adapter, and §8's links row (line 1244): `fw-ws-redis` holds its own `Tokio`
  under the links' rule, its subscription spawned and its reconnect slept on that runtime, its
  publish run through `Tokio::run`; with no runtime its publish fails and its subscription ends,
  logged, naming `.with_handle(..)` (F367); and the crate's two-process test against Redis
  (decision 9).
- The X table (lines 1314-1317): a row for `with_handle` on the seven links and `fw-ws-redis`, `Tokio::{try_current,
  handle, run}` and `Stopped`, and `fw-smol`.

## Needs sign-off

### S1. A link captures with `try_current` and refuses at first use

Decision 1: a link built outside a runtime holds none until `with_handle`, and refuses in `prepare`
and `connect`, naming `.with_handle(..)`, rather than panicking where it was built as
`Tokio::current()` does. The response's wording, "the way `Tokio::current()` does", reads as the
panic. The alternatives are a panicking constructor, which makes `new(..).with_handle(h)` unusable
off a runtime, with a second constructor per link taking the handle; or a constructor that takes
the handle on every link.

### S2. `Tokio::run`, `try_current`, `handle` and `Stopped` are public in `ulo-tokio`

Decision 3: the seven links share one helper, and gain `ulo-tokio` as a dependency. The alternatives
are the helper copied privately into each link, or a crate of its own for tokio-based links.

### S3. The result comes back through the task's `JoinHandle`

Decision 3: polled without a runtime context, carrying a panic and an abort. The alternatives are a
`futures-channel` oneshot beside an abort guard, the brief's wording taken literally, which repeats
what the `JoinHandle` already does; or entering the handle around each poll of the link's future
(`async-compat`'s approach), which spawns nothing per call but polls tokio's resources on the
caller's executor, which the response's "runs its I/O there" rules out.

### S4. What runs through `run`, and a task per frame on four links

Decision 1 and 4: everything needing tokio's context, and a library future not read to need none,
runs through `run`; a command handed to a task already on the link's runtime stays inline (MQTT,
NATS, TCP's writes). UDP, Redis, RabbitMQ and Kafka spawn one task per frame sent, on the client's
`send` and on the server's reply path. A consequence: a call dropped while its request's `send` task
is running sends its `cancel` from a task of the client's runtime, which can reach the broker before
the request does; the server then drops the `cancel`, which names no call it holds, and answers the
request to a client that no longer waits. Inline, the request was queued or not before the `cancel`
was created. The alternatives are reading lapin's and redis's futures far enough to keep their
publishes inline where they are handoffs, or every library future through `run`, which moved
MQTT's UNSUBSCRIBEs off the drain's first poll (decision 4).

### S5. TCP's and UDP's reply lanes are read by a task into 64 frames

Decision 1: the socket is read on the link's runtime, read ahead by up to 64 frames where the
caller's poll read one at a time. The alternatives are an unbounded queue, or one frame.

### S6. The F363 test is a conformance scenario

Decision 5: one scenario stamped into every link's suite, which adds a scenario to every suite's
count (TCP 26 to 27). The alternative is a test file in each link crate, which the brief's "once per
link" may have meant, with each broker crate starting its own container for it.

### S7. `Smol` takes an `Executor` the caller runs

Decision 6. The alternative is smol's global executor, through the `smol` crate and its threads.

### S8. The four-thread harness

Decision 7. The alternative is the test's thread alone, which never polls a task on a thread other
than the one aborting it.

### S9. `ulo-ws-redis` with no runtime fails its publishes and ends its subscription, logged

Decision 8: the `BroadcastAdapter` SPI has no `prepare`, so the refusal the links give at
`listen()` arrives here as each publish's error and as a subscription stream that ends at once
after a `tracing::error!`; an app binds with no broadcast reaching it. The alternative is a
defaulted `BroadcastAdapter::prepare(&self) -> Result<(), BoxError>` the hub calls from a server's
`prepare`, a change to the SPI.

### S10. The second process is the test binary run again, and the CI job keeps its name

Decision 9. The alternatives are two apps in one process, which share one `NodeId`, or a separate
binary target for the peer; and renaming the job `conformance (brokers)` now that it runs a
WebSocket test, which a branch protection rule naming the job would need to follow.

## Verification

Full, unfiltered output of every run is in the session scratchpad, `runtime-e1/`: `runs/`,
`suites/` (with `summary.txt`), `broken/` (with `summary.txt`), `specs/`, `scripts/`, `tree/` (with
`summary.txt`) and `verify/`.

- **Conformance**, three consecutive runs per suite, one crate at a time, the brokers with
  `--features integration --test conformance --locked`, test time as libtest reports it
  (`suites/summary.txt`):

| Suite | Run 1 | Run 2 | Run 3 | Result |
| --- | --- | --- | --- | --- |
| `ulo-rpc-tcp` conformance | 1.31 s | 1.31 s | 1.31 s | 27 passed |
| `ulo-rpc-tcp` conformance_cbor | 1.31 s | 1.31 s | 1.31 s | 27 passed |
| `ulo-rpc-tcp` conformance_tls | 1.32 s | 1.31 s | 1.31 s | 27 passed |
| `ulo-rpc-udp` conformance | 1.31 s | 1.31 s | 1.31 s | 26 passed, 1 ignored |
| `ulo-rpc-nats`, after decision 4 | 3.52 s | 5.48 s | 2.94 s | 27 passed |
| `ulo-rpc-redis` | 5.65 s | 5.54 s | 5.35 s | 28 passed: the suite's 27 and the link test |
| `ulo-rpc-mqtt`, after decision 4 | 6.05 s | 5.54 s | 5.58 s | 32 passed: the suite's 27 and five link tests |
| `ulo-rpc-rabbitmq`, `ULO_CONFORMANCE_PARALLEL=6` | 31.02 s | 31.53 s | 30.40 s | 27 passed |
| `ulo-rpc-kafka` | 42.90 s | 28.70 s | 25.26 s | 27 passed |

  Before decision 4, NATS passed three runs (3.67 s, 2.98 s, 3.21 s) and MQTT failed one test in
  each of three (decision 4). The TCP, UDP, Redis, RabbitMQ and Kafka sources did not change after
  their runs but for a doc comment on `Kafka`, edited before Kafka's runs began, and `Tokio`'s doc.
- **`ulo-ws-redis`'s two-process test**, `--features integration --test integration --locked`,
  three consecutive runs on the final tree: 1 passed and `peer_instance` ignored each, 0.52 s,
  0.33 s and 0.32 s (`suites/ws-redis-{1,2,3}.txt`, `verify/final2-summary.txt`); every container
  it started removed, and no second process left running (`pgrep`).
- **Each crate's tests,** `cargo test -p <crate> --locked`, once each on the tree before the F367
  addition (`verify/crates-summary.txt`) and `ulo-ws-redis` after it: `ulo-rpc-tcp` 89 passed and 1
  ignored over 10 binaries (the three suites, `client_close`, `deadline_answers`, `drain_end`,
  `late_goaway`, `handle`); `ulo-rpc-udp` 30 and 2; `ulo-rpc-nats`, `-redis`, `-mqtt` and
  `-rabbitmq` 1 each, their `handle` test, the suites compiling to nothing without `integration`;
  `ulo-rpc-kafka` 4 (3 unit tests and `handle`); `ulo-rpc` 16 and 5 ignored; `ulo-tokio` 27
  (`app` 9, `conformance` 12, `handle` 6) and its ignored doc example;
  `ulo-runtime-conformance` its ignored doc example; `ulo-smol` 12 and 1 ignored;
  `ulo-rpc-conformance` 2 and 2 ignored; `ulo-ws-redis` 1, its `handle` test.
- **The tree checks** (`tree/summary.txt`): `cargo tree -p <crate> -e normal -i tokio` prints
  nothing on stdout for `ulo`, `ulo-transport`, `ulo-net`, `ulo-http` (default features),
  `ulo-rpc`, `ulo-ws`, `ulo-graphql-ws`, `ulo-graphql-http`, `ulo-ws-conformance`,
  `ulo-runtime-conformance` and `ulo-smol`, each exiting 101 with "did not match any packages" or 0
  with "nothing to print", tokio being a dev-dependency of the second group; with `--all-features`
  it prints nothing for `ulo`, `ulo-transport`, `ulo-net`, `ulo-rpc`, `ulo-ws`, `ulo-graphql-ws`,
  `ulo-graphql-http`, `ulo-ws-conformance` and `ulo-smol`; `ulo-http` was checked at its default
  features alone, as the brief names it, its `tokio-io` feature bringing tokio by design. The
  planted check for `ulo-smol` printed tokio (the breaks table), and `ulo-rpc-tcp`'s tree prints
  tokio as a positive control.
- `cargo check --workspace --all-targets --all-features` with
  `CFLAGS=-I/opt/homebrew/opt/openssl/include LDFLAGS=-L/opt/homebrew/opt/openssl/lib`, and with
  defaults: exit 0, the 17 known warnings in `crates/ulo/src` and no warning location elsewhere, on
  the final tree (`verify/final2-check-*.txt`).
- `cargo +1.88 check --workspace --all-targets --exclude ulo-http-salvo --exclude
  ulo-graphql-async-graphql`: exit 0, the same 17 and no other; `ulo-smol`, `async-io` 2.6 and
  `polling` 3.11 build on 1.88.
- `cargo test --workspace --no-fail-fast` with the OpenSSL flags, on the final tree: 612 passed, 0
  failed, 71 ignored across 156 test binaries (`verify/final2-workspace-test.txt`): d3's 582, the
  scenario on TCP's three suites and UDP's (4), `ulo-tokio`'s four `run` tests, the seven links'
  `handle` tests (9) and `ulo-ws-redis`'s (1), and `ulo-smol`'s twelve. The ignored one added is
  `ulo-smol`'s `ignore` doc example; the binaries added are the eight `handle` targets,
  `ulo-ws-redis`'s `integration` target, empty without its feature, and `ulo-smol`'s three. The run
  before the F367 addition gave 611, 71 and 154.
- `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --lib -p <crate>` for `ulo-tokio`, `ulo-smol`,
  `ulo-rpc`, `ulo-rpc-conformance`, the seven links and `ulo-ws-redis`: each exits 0
  (`verify/doc-*.txt`); a planted link to a missing item failed it (the breaks table).
- `cargo +1.98.1 clippy` over `ulo-tokio`, `ulo-smol`, `ulo-rpc`, `ulo-rpc-conformance` and the
  seven links with `--all-targets --no-deps --locked`, and over `ulo-ws-redis` with `--features
  integration` too: exit 0; 30 warning locations in the first run and none in `ulo-ws-redis`'s,
  none on a line this stage changed or in a new file, by `scripts/changed_lines.py` reading `git diff
  -U0` and the untracked files; run first against three planted locations, two on changed or new
  lines, it reported those two (`verify/checker-selftest.txt`).
- **Containers:** the user's four, seaweedfs, mailpit, postgres:18 and redis:7, untouched and the
  only ones running after every suite run (`suites/summary.txt` counts 4 after each). Every
  container the broker suites and the Redis test started was removed by the test that started it.
  No permission check refused an action, and the Docker engine answered throughout.
