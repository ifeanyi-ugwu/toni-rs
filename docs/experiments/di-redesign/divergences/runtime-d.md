# Divergences: runtime neutrality, stage d: `ulo-ws` on `async-tungstenite` and the app's runtime, the standalone server behind `tokio-server`, `ulo-graphql-ws` off tokio, the core's `RuntimeMissing`, and `RpcClient::timeout`

The thirty-fourth response splits runtime neutrality into six stages; this is the fourth. It also
builds S1, S2 and S7 of the thirty-sixth response. At default features, `ulo-ws`,
`ulo-graphql-ws` and `ulo-graphql-http` have no tokio in their normal trees, which closes F362's
tokio clause. `ulo-ws` speaks the protocol through `async-tungstenite` over `futures-io`, runs
every task on the app's `Runtime` (a connection, its messages, a hand-written gateway's inbox,
broadcast delivery, `AfterInit`), races its futures without a runtime, and wakes its queues
through `event-listener`; its in-memory broadcast adapter is `async-broadcast`. The standalone
server, whose accept loop and HTTP/1.1 handshake are hyper's and `ulo-hyper-serve`'s on tokio,
sits behind a default-off `tokio-server` feature; once a connection is upgraded it runs on the
app's runtime like one on the HTTP server's port. `ulo-graphql-ws` spawns its two tasks through
the new `Connection::runtime`. `listen()` refuses an app that binds any transport with no runtime
as `RuntimeMissing`, which replaces `TimerMissing`, and the RPC server's own refusal is gone.
`RpcClient` gains `.timeout(Bound)`, and a wiring error for a missing `dyn Runtime` names
`.runtime(..)`.

Files changed: the workspace `Cargo.toml` and `Cargo.lock`; `crates/ulo/src/{lib.rs,
app/mod.rs, app/handle.rs, error/mod.rs, error/wiring.rs, transport/server.rs}`;
`crates/ulo-ws/{Cargo.toml, src/lib.rs, src/broadcast.rs, src/connection.rs, src/envelope.rs,
src/handoff.rs, src/module.rs, src/rooms.rs, src/server.rs, src/watch.rs (new),
tests/attributes.rs, tests/limits.rs, tests/support/mod.rs, tests/broadcast.rs (new),
tests/runtime.rs (new)}`; `crates/ulo-graphql-ws/{Cargo.toml, src/gateway.rs, tests/runtime.rs
(new)}`; `crates/ulo-rpc/{Cargo.toml, src/client.rs, src/client_module.rs, src/server.rs,
tests/runtime.rs}`; `crates/ulo-tokio/tests/app.rs`; `crates/ulo-http/{src/embed.rs,
tests/max_inflight.rs, tests/post_deadline_body.rs, tests/stream_end.rs}`;
`crates/ulo-http-hyper/tests/route_timeout.rs`; `crates/ulo-http-conformance/src/lib.rs`;
`crates/ulo-http-{axum,actix,poem,rocket,salvo}/src/lib.rs` (doc only);
`crates/ulo-codegen-tests/tests/{grpc.rs, support/mod.rs}`; F362 appended and F367 filed in the
workspace's `FRAMEWORK_GAPS.md`. The tree is `9d8b42dc` plus this stage.

## The signatures

```rust
// ulo, the core
#[non_exhaustive]
#[derive(Debug)]
pub struct RuntimeMissing { pub transport: TypeName }     // replaces `TimerMissing`, removed
impl AppHandle {
    pub fn mounted<T: Transport>(&self) -> Result<Vec<MountedHandler<T>>, RuntimeMissing>; // was `TimerMissing`
}
impl<T: Transport> Mounted<'_, T> {
    pub fn runtime(&self) -> &Arc<dyn Runtime>;          // new
}

// ulo_ws
#[cfg(feature = "tokio-server")]
pub struct Server { /* unchanged */ }                    // was unconditional
impl Connection {
    pub fn runtime(&self) -> &Arc<dyn Runtime>;          // new, beside `timer`
}

// ulo_rpc
impl RpcClient {
    pub fn timeout(self, timeout: Bound) -> Self;        // new; panics on `Bound::After(Duration::ZERO)`
}
```

`TimerMissing`'s `Display` was "transport `T` is bound on an app with no `Timer`; set one with
`.timer(..)`"; `RuntimeMissing`'s is "transport `T` is bound on an app with no `Runtime`; set one
with `.runtime(..)`, which sets the app's timer too". `App<Connected>::listen`, `Server`,
`WiringError` and every `ulo-ws` type besides `Server` and `Connection` are unchanged in shape.
`ulo_ws::Server` compiles only with `ulo-ws`'s `tokio-server` feature. No handler, module or
controller signature changes, and no macro's output changes.

Private: in the core, `mounted_parts` answers the runtime in place of the timer and refuses with
`RuntimeMissing`, `Mounted` carries both, and `wiring.rs` gains `builder_help`. In `ulo-ws`,
`Tracker::spawn` takes the runtime, `Tracker` holds `TaskHandle`s and three `Watch`es, `Hub::start`
and `after_init` take a `&dyn Spawn`, `Accept::timer` became `Accept::runtime`, `ConnInner` gains
`runtime`, the read loop's wait is a `Turn` over `BRANCHES`, and `watch.rs` is new. In `ulo-rpc`,
the default timeout moved from `ClientInner` to the `RpcClient` handle, with a `timeout_of` helper.

Dependencies:

- **`ulo-ws`** loses `tokio` (`sync`, `macros`, `rt`), `tokio-tungstenite` and its unconditional
  `ulo-http/tokio-io`, with the comment that named stage d. It gains `async-tungstenite` 0.33
  (`handshake`, `futures-03-sink`), `async-broadcast` 0.7, `event-listener`, `futures-channel` and
  `futures-io`. `hyper`, `hyper-util`, `http-body-util`, `ulo-hyper-serve` and `ulo-net` become
  optional, enabled with `ulo-http/tokio-io` by `tokio-server`. Its dev-dependencies gain
  `ulo-ws` itself with `tokio-server`, and tokio's `macros` and `sync`, which the normal
  dependency used to supply.
- **`ulo-graphql-ws`** loses `tokio`; it gains dev-dependencies on `ulo-http-hyper`, `ulo-tokio`,
  `tokio` and `tokio-tungstenite` for its first test.
- **`ulo-rpc`**'s tokio dev-dependency gains `test-util`.
- **The workspace** gains `async-broadcast = "0.7"` and
  `async-tungstenite = { version = "0.33", default-features = false }`; `Cargo.lock` gains those
  two packages and no other, every dependency of theirs being in the lock already.

## Decisions

### 1. Each tokio use in `ulo-ws` and what replaced it

| Use | Where | Replacement |
| --- | --- | --- |
| `tokio_tungstenite::WebSocketStream`, `tungstenite` types | `connection.rs`, `envelope.rs` | `async_tungstenite::WebSocketStream` and its `tungstenite` re-export, the same tungstenite 0.28 (decision 2) |
| `tokio::io::{AsyncRead, AsyncWrite}` | `answer`, `run`, `serve`, `refuse`, `write` bounds | `futures_io::{AsyncRead, AsyncWrite}`; the hand-off's `Upgraded` implements them, and the standalone server wraps hyper's upgrade as `Upgraded::from_tokio(TokioIo::new(io))` |
| `tokio::spawn`, `AbortHandle` map | `Tracker::spawn`, `close` | `runtime.spawn(..)` into a map of `TaskHandle`s. `close` takes the map out of its lock before aborting, since an abort may drop a task's future on the calling thread and its guard takes that lock |
| `tokio::spawn`, `AbortHandle` | `Hub::start`, `stop` | `runtime.spawn(..)`, the `TaskHandle` kept and aborted at `stop` |
| `tokio::spawn` | `after_init` | `runtime.spawn(..)`, the handle dropped, which detaches |
| `JoinSet<()>` and `shutdown().await` | `serve`, `received`, `raw_inbox` | `TaskSet` over the app's runtime; `abort_all()` then `join_all().await` |
| `tokio::select!` unbiased, eight branches | `serve` | a `poll_fn` over eight branches, each turn starting at the next branch (decision 5) |
| `tokio::select!` unbiased, two branches | `refuse` | a `poll_fn` alternating which branch is polled first |
| `tokio::select!` biased: cancellation, item | `pump` | `future::select(pin!(exec.cancelled()), stream.next())`, which polls its first future first |
| `tokio::select!` unbiased: connection, drain | `server.rs` `connection` | `future::select(served, draining.wait())`, then `graceful_shutdown` and the connection to its end (decision 5) |
| `Notify` with `notify_one`, `notify_waiters`, `enable` | `Outbound::wake`, `Outbound::space`, `Flight::progress` | `event_listener::Event`, each waiter registering its listener before it reads the state it waits on (decision 4) |
| `watch::Sender` with `subscribe`, `borrow`, `wait_for` | `Tracker::{phase, token, live}` | a private `Watch` over a `std` mutex and an `event-listener` event, as `ulo-rpc`'s (decision 4) |
| `mpsc::unbounded_channel` | a hand-written gateway's inbox | `futures_channel::mpsc::unbounded`, `unbounded_send`, the receiver read with `StreamExt::next` |
| `broadcast::channel(1024)` | `InMemory` | `async-broadcast` in overflow mode (decision 3) |
| `hyper_util::rt::{TokioIo, TokioTimer}`, `ulo-hyper-serve` | `server.rs` | unchanged, behind `tokio-server` (decision 6) |
| `#[tokio::test]`, `tokio::time`, `tokio-tungstenite`'s client | tests | unchanged, dev-dependencies |

### 2. `async-tungstenite` 0.33, not the newest 0.35

- **What exists:** 0.35.0 (2026-07-28, MSRV 1.85) is built on tungstenite 0.30; 0.34.x on 0.29;
  0.33.0 (2026-02-20, MSRV 1.85) on tungstenite 0.28, the version the branch already ran through
  `tokio-tungstenite` 0.28 and still runs in the tests' client.
- **The choice:** 0.33. The move then changes the I/O traits and nothing in the protocol: the
  message types, error variants, `WebSocketConfig` and `derive_accept_key` are the ones the code
  already used, and the tree carries one tungstenite. 0.35 would put a tungstenite upgrade in
  the same change and a second tungstenite in the test tree. See S1.
- **Features:** `default-features = false` with `handshake`, for `derive_accept_key`, and
  `futures-03-sink`, for `SinkExt`'s `feed`, `flush`, `send` and `close`. No runtime feature is
  enabled, so the crate brings `futures-io`, `futures-util`, `atomic-waker`, `log` and
  tungstenite alone.

### 3. `async-broadcast` for the in-memory adapter

- **What `InMemory` needs:** a sender that never waits and never fails for want of subscribers,
  a subscriber that starts at the next broadcast, and a subscriber more than 1,024 broadcasts
  behind skipping the oldest and learning how many it missed, which the adapter logs at `warn`.
- **How `async-broadcast` gives it:** `set_overflow(true)` makes a full queue drop its oldest
  broadcast rather than refuse the new one, and a subscriber that had not read it receives
  `RecvError::Overflowed(n)`, tokio's `Lagged(n)`. An `InactiveReceiver` keeps the channel open
  while no subscriber is active, holding no broadcast; `Sender::new_receiver` starts at the next
  broadcast, as tokio's `subscribe` does. `try_broadcast` answers `Inactive` with no active
  subscriber, which the adapter ignores, as it ignored tokio's `send` error.
- **Why it:** it is runtime-free, built on `event-listener`, already in the lock, and has MSRV
  1.60. The alternatives in the lock carry messages to one receiver each (`async-channel`,
  `futures-channel`), so a broadcast over them is a list of per-subscriber channels with an
  overflow policy written here.

### 4. Wake-ups without a permit, and the tracker's watch

- **The difference that matters:** tokio's `Notify::notify_one` leaves a permit for a waiter
  that has not started waiting; `event_listener::Event::notify` wakes only listeners registered
  already. Every wait therefore registers before it reads: `push_wait` listens on `space` before
  checking for room, and the read loop holds a listener on `wake`, on `progress` and, until the
  drain, on the tracker's phase, each registered before the turn reads the queue, the counts or
  the phase. A listener is kept until it is heard and registered again on the next turn, so a
  turn allocates only after a wake.
- **The tracker:** `phase`, `token` and `live` are each a private `Watch`
  (`crates/ulo-ws/src/watch.rs`), `ulo-rpc`'s shape with a `changed()` listener added for the read
  loop. A dropped sender, which tokio's `changed` reported as `Err` and the loop read as the
  drain, cannot happen: the loop's `Accept` holds the tracker. See S4.

### 5. The selects: bias and cancellation

- **The read loop** (`serve`, `crates/ulo-ws/src/connection.rs`) waits on eight branches: the
  socket while it reads, the three listeners, the ping, pong and close clocks, and the next
  message task's end while any runs, each with the condition tokio's `select!` gave it. tokio's
  unbiased `select!` starts at a random branch; the loop starts each turn at the next branch in
  turn, so no branch that is always ready starves another. The wait is a `poll_fn` inside a block,
  so every branch's future, `join_next`'s included, is dropped before the event is handled, and
  each arm's body writes to the socket the wait borrowed. `join_next` dropped unanswered takes
  nothing out of the set.
- **`refuse`** alternates which of the socket and the deadline it polls first, so a client that
  keeps sending cannot hold it past its deadline.
- **`pump`** polls the cancellation before the stream, as the biased `select!` did.
- **The standalone server's connection** (`server.rs`) polls the connection before the drain
  signal; on the signal it starts the graceful shutdown and awaits the connection to its end.
  When both are ready the connection has ended, and a graceful shutdown of an ended connection
  stops nothing, so the order changes no outcome.

### 6. The standalone server: (a), behind `tokio-server`

- **How it is built:** an HTTP/1.1 server over `ulo-hyper-serve`, whose accept loop adopts
  `ulo-net`'s listeners into tokio, handshakes TLS through `tokio-rustls` and spawns each
  connection into a tokio `JoinSet`, and over hyper's `http1::Builder` driven through
  `TokioIo` and `TokioTimer`. That is the HTTP backend's arrangement in `ulo-http-hyper`.
- **(b), a socket the caller supplies through `ulo-net`'s `std::net` types and an async adapter
  from the runtime crate,** needs each runtime crate to provide an accept, a read and a write: a
  socket interface beside `Runtime`, which answer 2 keeps out. The server would also need an
  HTTP/1.1 parser for the upgrade request and TLS on `futures-io`, a second `ulo-hyper-serve`.
- **(a), as built:** `Server` and its module compile with `ulo-ws`'s `tokio-server` feature,
  which enables hyper, hyper-util, http-body-util, `ulo-hyper-serve`, `ulo-net` and
  `ulo-http/tokio-io`. What runs on tokio is the accept and the handshake up to the 101: the
  upgraded I/O crosses into `ulo-ws` as a `futures-io` stream, and the connection's task and every
  task it starts are spawned on the app's runtime from `Mounted::runtime()`, as on the HTTP port.
- **A feature, not a crate:** the server uses `Accept`, `Tracker`, `answer`, `build_table`,
  `check_defaults`, the `Hub` and `HubMeta`, all crate-private. A crate of its own would need
  them as a doc-hidden SPI. `ulo-ws`'s own tests serve most gateways on this server; they enable
  the feature through a dev-dependency of `ulo-ws` on itself.
- **The name** follows `ulo-http`'s `tokio-io`: it names the runtime it brings. A smol
  standalone server, which stage e does not include, would sit beside it. See S2.

### 7. The hand-off, the reply tracking and the bound

- **The same-port path** reads `ulo_http::Upgraded`, on `futures-io` since stage b, with no
  conversion. The hand-off takes the app's runtime from `AppHandle::runtime()` in `prepare` and
  derives the connection's timer from it; the lookup of `dyn Timer` at the first upgrade, which
  existed because `prepare` cannot await, is gone.
- **Batch 19's F358 move is kept:** `message` wraps a `Reply::Many` stream in `Tracked` in the arm
  that hands it to `pump`, and `stream_end.rs`'s three tests pass unchanged.
- **`max_inflight` is kept:** `Limits::resolve` is untouched, so it is 1,024 messages per
  connection at `Count::Default`; `limits.rs`'s `max_inflight` tests pass unchanged.

### 8. S1: `RuntimeMissing`, one core check

- **The refusal:** `listen()` refuses an app that queues any server with no runtime, before any
  `prepare`, as `StartupError::Bind { transport, source }` with a `RuntimeMissing` in its
  `source` (`crates/ulo/src/app/mod.rs`, `listen`). An app given `.timer(..)` alone is refused;
  one that queues no server, a job or a CLI command, passes `listen()` with the timer alone.
- **It replaces `TimerMissing`:** a runtime is a timer, so the old refusal is a subset. `ulo`
  exports `RuntimeMissing` in `TimerMissing`'s place.
- **`AppHandle::mounted` and `mounted_parts`** refuse the same way, so an upgrade handler reading
  its handlers sees the refusal `listen()` would have given. `Mounted` carries the runtime and
  `Mounted::runtime()` answers it without an `Option`, since no server is prepared without one;
  the RPC, WebSocket and core test servers read it there.
- **The RPC server's refusal** in `prepare` is removed, with its `let`-`else` after
  `into_result`, its paragraph in the server's doc, and the test that pinned it, now pinning the
  core's refusal through the same RPC app.
- **An embedding** is a `Server` that binds no socket and is refused too: S1 covers every
  transport bound, and every one now spawns through the runtime or hands its upgrades to code that
  does.
- **Every app that binds a transport** moved from `.timer(ulo_tokio::Timer)` to
  `.runtime(ulo_tokio::Tokio::current())`: `ulo-http-conformance`'s app, which every adapter's
  conformance suite runs; `ulo-ws`'s test support and `attributes.rs`; `ulo-http`'s
  `max_inflight`, `stream_end` and `post_deadline_body`; `ulo-http-hyper`'s `route_timeout`;
  `ulo-codegen-tests`' `grpc.rs` and gRPC support. The six adapter crates' and `embed.rs`'s doc
  examples changed the same way. Apps that bind nothing keep `.timer(..)`: UDP's `close_read`,
  `ulo-codegen-tests`' client apps in `clients.rs`, and `ulo-tokio`'s `app.rs`.
- **The `ulo dev` templates** (`crates/ulo-cli/src/templates/new`) are written against the
  published `ulo` 0.2.0 they pin (`UloFactory`, `AxumAdapter`) and show no `.timer(..)`; they are
  unchanged.

### 9. S2: `RpcClient::timeout`

- **Where it lives:** on the `RpcClient` handle, not the shared `ClientInner`. `timeout(self,
  Bound)` consumes the handle and returns it, so the handle and clones made from it after carry
  the new default, and clones made before keep theirs while sharing the connection.
  `RpcClientModule::timeout` reaches the same field through `of_module`.
- **Its values** are the module's: five seconds at `Bound::Default`, `After(d)`, and no timeout at
  `Bound::Unbounded`; a call's own `.timeout(d)` still wins.
- **A zero panics** at the call, with the module's wording: "`RpcClient::timeout(Bound::After(
  Duration::ZERO))` on the <link> link would time out every call; write `Bound::Unbounded` to turn
  the timeout off". The module refuses a zero at `wire()`; a client built in `main` has no step
  before its first call. See S3.

### 10. S7: the wiring error names `.runtime(..)`

- **Where:** in the core's rendering of `WiringError::Missing` (`crates/ulo/src/error/wiring.rs`,
  `builder_help`). For the key `dyn Runtime` the help line is "help: set one on the app with
  `.runtime(..)`, which sets its timer too", and for `dyn Timer` "help: set one on the app with
  `.timer(..)`, or with `.runtime(..)`, which sets both", in place of "import a module that
  exports `dyn Runtime`, or provide it in RpcClientModule", which pointed at a binding no module
  can provide.
- **Why there and not in `RpcClientModule`:** a module registers before the app's builder is
  known, so it cannot tell a timer-only app from one with a runtime; the factory reading
  `Dep<dyn Runtime>` is what fails. The change covers every binding that reads either key, not
  only the RPC client's; the variant's fields are unchanged. The message a timer-only app with an
  `RpcClientModule` gets:

```text
error: wiring failed with 1 error

  × missing dependency `dyn Runtime`
    ├─ needed by RpcClient factory (param #1) in RpcClientModule
    └─ help: set one on the app with `.runtime(..)`, which sets its timer too
```

### 11. `ulo-graphql-ws`

- **Its two spawns,** the `connection_init_timeout` watch in `on_connect` and each `subscribe`'s
  operation, are `conn.runtime().spawn(..)` with the handle dropped, which detaches as dropping a
  `JoinHandle` did (`crates/ulo-graphql-ws/src/gateway.rs:219` and `:336`). A hand-written gateway
  reaches the runtime through `Connection::runtime()`, added beside `Connection::timer()`.
- **Nothing else pulled tokio:** its only route was `ulo-ws`, and `ulo-graphql-http`'s only route
  to tokio was `ulo-graphql-ws`. `ulo-graphql-http` still depends on `ulo-graphql-ws`, and so on
  `ulo-ws`, whether or not an app mounts subscriptions; F362's feature-gating candidate is not
  built here.

### 12. Left as they are

- **The seven links** and their 26 `tokio::spawn` calls (stage e, S8 and F363).
- **`ulo-ws-redis`** spawns its subscription on the ambient tokio runtime and has no test; filed
  as F367 for stage e.
- **A hand-written gateway always spawns its `AfterInit`,** the trait's default doing nothing:
  its `ConnectHandler` stores the hook unconditionally (`crates/ulo-ws/src/gateway.rs:561`). One
  detached task per such gateway at bind, which the `ulo-graphql-ws` tests count.

## The tests

- **`crates/ulo-tokio/tests/app.rs`, four new,** over a test-local transport `Bare` and a
  `BareServer` that binds nothing and records the runtime its `prepare` was handed:
  - `an_app_with_a_timer_alone_is_refused_when_it_binds_a_transport`: `listen()` answers
    `StartupError::Bind` for transport `Bare`, its source downcasts to `RuntimeMissing`, its text
    names the missing `Runtime` and `.runtime(..)`, and the server was never prepared.
  - `an_app_without_a_clock_is_refused_when_it_binds_a_transport`: the same with no clock.
  - `a_runtime_is_the_one_a_server_is_handed`: `Mounted::runtime()` is the object
    `AppHandle::runtime()` answers.
  - `a_timer_alone_is_enough_for_an_app_that_binds_nothing`: a timer-only app with no server
    passes `listen()` and shuts down.
- **`crates/ulo-rpc/tests/runtime.rs`, three new and one rewritten,** over its `Observed` link,
  which connects at once and never replies:
  - `listen_refuses_an_rpc_server_on_an_app_with_no_runtime`, which was
    `the_server_refuses_an_app_with_no_runtime`: the refusal is `StartupError::Bind` with
    `RuntimeMissing`, naming `.runtime(..)`.
  - `a_client_module_on_an_app_with_a_timer_alone_fails_wiring_naming_runtime` (S7): the wiring
    error names `dyn Runtime` and the `.runtime(..)` help.
  - `a_client_built_outside_an_app_times_out_at_its_own_timeout` (S2), on a paused clock: a
    request on a client from `RpcClient::new` fails `Timeout` at exactly 5 s, at 250 ms after
    `.timeout(Bound::After(250 ms))`, not within a minute after `.timeout(Bound::Unbounded)`, and
    at 5 s again on the client whose clones were given those timeouts.
  - `a_zero_client_timeout_is_refused_where_it_is_written` (S2): the panic, by its message.
- **`crates/ulo-ws/tests/runtime.rs`, two,** on an app whose runtime counts what it is handed:
  on the standalone server and through the hand-off, broadcast delivery and `AfterInit` are the
  two tasks spawned at bind, the connection is the third once its 101 arrives, and its message
  the fourth.
- **`crates/ulo-ws/tests/broadcast.rs`, two,** on `InMemory` alone: a subscriber receives, in
  order, what is published after it subscribed and not the publish made before with no
  subscriber; one 1,030 broadcasts behind receives broadcast 6 first, then up to 1,029, then the
  next one published.
- **`crates/ulo-ws/tests/limits.rs`, one new,**
  `a_streamed_answer_longer_than_max_outbound_waits_for_room_and_is_written_whole`: on the
  `max_outbound = 1` gateway, a 50-item stream arrives whole and in order with `complete`, the
  connection then answers an `echo`, and the server closed nothing.
- **`crates/ulo-graphql-ws/tests/runtime.rs`, two, the crate's first,** over an engine whose
  subscriptions yield two events, the gateway on the HTTP port through the hand-off, and a
  counting runtime:
  - `a_subscription_streams_on_the_app_s_runtime`: after `connection_ack`, five tasks (`ulo-ws`'s
    four and the init watch); after `next`, `next` and `complete`, six.
  - `the_init_timeout_closes_the_connection_from_the_app_s_runtime`: at a 100 ms init timeout the
    connection closes with 4408 "Connection initialisation timeout", and five tasks were spawned.
- **Changed:** `ulo-ws`'s test support gains `Running::start_on(root, server, runtime)`;
  `start` delegates to it with `Tokio::current()`.

### Before and after

Each break rewrote one or more spans through `runtime-d/brk.py`, which asserts each span occurs
once, runs the target, writes every file back byte for byte and compares hashes; every restore
reported `ok`, and the tree's diff hash was the same before and after each round. The specs are in
`runtime-d/specs/`, the full output of each run in `runtime-d/broken/`. "Ambient tokio" breaks add
`tokio` to the crate's dependencies and call `tokio::spawn` in place of the runtime.

| Break | Target | Result |
| --- | --- | --- |
| `Tracker::spawn` on ambient tokio | `ulo-ws --test runtime` | both failed at the connection's count: 2 against 3 |
| a message's task on ambient tokio | same | both failed at the message's count: 3 against 4 |
| broadcast delivery on ambient tokio | same | both failed at bind: 1 against 2 |
| `take` no longer wakes `space` | `ulo-ws --test limits` | the 50-item stream failed, "the next message did not happen within 5s"; the other nine passed |
| `InMemory` without overflow mode | `ulo-ws --test broadcast` | the lagging test failed, broadcast 0 first against 6; the other passed |
| `listen()`'s runtime check disabled | `ulo-tokio --test app` | both refusal tests failed, "was not refused at `listen()`"; seven passed |
| `listen()` checking the timer, `TimerMissing`'s condition | same | the timer-alone refusal failed; eight passed |
| `listen()` refusing a timer-only app that binds nothing | same | `a_timer_alone_is_enough..` failed, "an app binding no transport was refused"; eight passed |
| `Mounted::runtime` a wrapper of the app's runtime | same | `a_runtime_is_the_one_a_server_is_handed` failed, two addresses; eight passed |
| `RpcClient::timeout` ignoring its argument | `ulo-rpc --test runtime` | the timeout test failed: 5 s against 250 ms; five passed |
| the last `timeout` applying to every clone | same | the timeout test failed at the client whose clones were given timeouts: `None` against 5 s |
| a zero timeout accepted | same | the `should_panic` test failed; five passed |
| `builder_help` answering nothing | same | the S7 test failed, the message ending at "needed by RpcClient factory (param #1) in RpcClientModule"; five passed |
| the init watch on ambient tokio | `ulo-graphql-ws --test runtime` | both failed: 4 against 5 |
| a subscription's task on ambient tokio | same | the subscription test failed at its count, 5 against 6; the init test passed |
| the restored tree | every target above | every test passed; `ulo-ws` and `ulo-graphql-ws` three runs each |

## Left for the transports DESIGN fold

Line numbers are those read on `9d8b42dc`.

- §0, principle 6 (line 14), and the core DESIGN's principle 3 (line 11): `TimerMissing` in the
  list of condition-named errors is `RuntimeMissing`.
- §1's crate table: `fw-ws`'s row (line 32) gains "no runtime: every task on the app's
  `Runtime`; `Server` behind the `tokio-server` feature"; `fw-graphql-ws`'s (line 41) spawns on
  the app's runtime through `Connection::runtime`.
- §3.5 (line 489): `fw_ws::Server` is behind `tokio-server`; after the 101 the connection runs
  on the app's runtime. §3.7 (line 554) stays true of the three servers, the WebSocket one under
  its feature.
- §3.8's example (line 613) and the core DESIGN's §9.4 example (line 864) bind a server with
  `.timer(fw_tokio::Timer)`, which `listen()` now refuses: `.runtime(fw_tokio::Tokio::current())`.
- §4.1: `Connection::runtime()` beside `Connection::timer()`; the read loop's wait (decision 5)
  and its wake-ups (decision 4). §4.3: `InMemory` over `async-broadcast` in overflow mode.
- §5.3 (line 962): the server's `prepare` no longer refuses an app with no runtime and reads it
  through `Mounted::runtime()`; `listen()` refuses first.
- §5.4 (line 1003): `RpcClient::timeout(Bound)`, on the handle, a zero panicking; the wiring error
  of a timer-only app names `.runtime(..)`.
- §8 (line 1203): "An app with neither keeps a zero-length drain and `listen()`'s `TimerMissing`;
  `listen()` does not refuse an app without a runtime, and the RPC server ... refuses one in its
  own `prepare`" reads: `listen()` refuses an app that binds a transport with no runtime as
  `RuntimeMissing`, `.timer(..)` alone staying valid for an app that binds none. The runtime
  table (line 1224): `fw-ws`, `fw-graphql-ws` and `fw-graphql-http` move to the runtime-free row
  at default features; `fw-ws`'s `tokio-server` joins the tokio-bound row beside
  `fw-hyper-serve`.
- §10's failure table (line 1354): the RPC server's own row becomes the core's: an app binding
  any transport with no runtime, at `listen`, `StartupError::Bind` with `RuntimeMissing`. The core
  DESIGN's §9.5 (line 922), §10.2 (lines 981, 1011-1014), the paragraphs at lines 1183 and 1191,
  the failure table (line 1276) and the API table (line 1368) name `TimerMissing` and the
  timer-only condition.
- §11's SPI table: `RuntimeMissing`, `Mounted::runtime`, `Connection::runtime`,
  `RpcClient::timeout`; X20's row (line 1287) and paragraph (line 746) read `RuntimeMissing`.
- Decision 49 (line 1442) is superseded: the refusal is the core's, at `listen()`.

## Needs sign-off

### S1. `async-tungstenite` 0.33, a version behind the newest

Decision 2: 0.33 keeps tungstenite 0.28, the protocol library the branch already runs, so this
stage changes the I/O traits and not the protocol. The alternative is 0.35, with tungstenite 0.30
and the tests' client moved to match, in this stage or as its own change.

### S2. The standalone server behind a default-off `tokio-server` feature of `ulo-ws`

Decision 6: option (a). An app serving a `port = own` gateway enables the feature; `ulo-ws`'s
tests enable it through a dev-dependency on the crate itself. The alternatives are the server in
a crate of its own over a doc-hidden SPI, a feature with another name, or option (b).

### S3. `RpcClient::timeout` lives on the handle and panics on zero

Decision 9: clones made before the call keep their default, and a zero panics where it is
written. The alternatives are the timeout on the shared client, which `timeout(self, ..)` cannot
set once a clone exists, and a zero accepted as written, timing every call out, or answered as an
error.

### S4. `ulo-ws` carries its own copy of the watch

Decision 4: `ulo-ws` copies `ulo-rpc`'s `Watch` and adds a `changed()` listener, leaving
`ulo-rpc` untouched this stage. The alternative is one watch in `ulo-transport`, shared by both,
which the thirty-sixth response's S6 named.

### S5. The read loop starts each turn at the next branch

Decision 5: tokio's unbiased `select!` picks a random start; the loop rotates through the eight
branches, deterministic and starving none. The alternative is a random start, which needs a
source of randomness the crate does not have.

### S6. The missing-`dyn Runtime` help is the core's, for every reader

Decision 10: the help line changes for any binding reading `dyn Runtime` or `dyn Timer`, not only
`RpcClientModule`'s client. The alternative is a refusal written by the module, which cannot see
the app's builder.

## Verification

Full, unfiltered output of every run is in the session scratchpad, `runtime-d/`: `tree/`,
`suites/` (with `summary.txt`), `verify/` (`final/` for the last pass), `broken/`, `specs/` and
`runs/`.

- **The tree checks, first against violations.** On `9d8b42dc`, `cargo tree -p <crate> -e normal
  -i tokio` printed tokio for `ulo-ws` (18 lines), `ulo-graphql-ws` (20) and `ulo-graphql-http`
  (22) (`tree/before-*`). On the changed tree, `tokio` planted in `ulo-ws`'s dependencies made all
  three print it (`tree/planted-ulo-ws/`), and planted in `ulo-graphql-http`'s made that one print
  it and the other two print nothing (`tree/planted-ulo-graphql-http/`); the manifests and
  `Cargo.lock` were restored from copies, checked by hash. On the final tree
  (`tree/final-2/`), at default features, stdout is empty for every crate checked: `ulo-ws`,
  `ulo-graphql-ws`, `ulo-http` and `ulo-rpc` exit 0 with "nothing to print"; `ulo-graphql-http`,
  `ulo`, `ulo-transport` and `ulo-net` exit 101 with "package ID specification `tokio` did not
  match any packages", tokio being in none of their graphs, dev-dependencies included. The same
  with `--target all` (`tree/final-2-target-all/`). With `--all-features`, `ulo-http` prints
  tokio through `tokio-io` and `ulo-ws` prints it through hyper with `--features tokio-server`,
  which shows the check reports the opt-in features. No `tokio`, `async-std`, `smol`, `async-io`
  or `hyper` package is in the default normal trees of the three crates, by a grep of `cargo tree
  -e normal --prefix none` that names `hyper` and `tokio` for `ulo-ws` with `tokio-server`.
- **Tests:** `cargo test --workspace --no-fail-fast --locked` with the OpenSSL flags: 533 passed,
  0 failed, 66 ignored across 136 test binaries, counted from the `test result:` lines: batch
  19's 519, `ulo-tokio`'s four, `ulo-rpc`'s three, `ulo-ws`'s five and `ulo-graphql-ws`'s two.
  Per crate, `cargo test -p <crate> --locked`: `ulo-ws` 64 passed, 3 ignored (three runs);
  `ulo-tokio` 23, 1 ignored; `ulo-rpc` 16, 5 ignored; `ulo-codegen-tests` 53; `ulo-http` 20, 9
  ignored; `ulo-http-hyper` 44, 3 ignored; `ulo-http-axum` 38, 1 ignored; `ulo-http-actix` 34, 5
  ignored; `ulo-http-poem` 38, 1 ignored; `ulo-http-rocket` 38, 1 ignored; `ulo-http-salvo` 39, 2
  ignored; `ulo-rpc-tcp` 83, 1 ignored (its three suites, `client_close`, `deadline_answers`,
  `drain_end`, `late_goaway`); `ulo-rpc-udp` 28, 2 ignored; `ulo-graphql-ws` 2 (three runs).
  `ulo-ws-redis`, `ulo-graphql`, `ulo-graphql-http`, `ulo-graphql-async-graphql` and
  `ulo-graphql-juniper` have no tests beyond ignored doc examples; `ulo-ws-redis` starts no broker
  (F367).
- **Broker suites,** once each, one at a time, `--features integration --test conformance
  --locked`: NATS 26 passed (3.41 s); Redis 27 (5.34 s); MQTT 31 (5.85 s); Kafka 26 (27.08 s);
  RabbitMQ with `ULO_CONFORMANCE_PARALLEL=6` 26 (34.08 s).
- `cargo check --workspace --all-targets --all-features` with
  `CFLAGS=-I/opt/homebrew/opt/openssl/include LDFLAGS=-L/opt/homebrew/opt/openssl/lib`, and
  without `--all-features`: exit 0, the 17 known warnings in `crates/ulo/src` and no other.
- `cargo +1.88 check --workspace --all-targets --exclude ulo-http-salvo --exclude
  ulo-graphql-async-graphql`: exit 0, the same 17 and no other. `cargo +1.88 check -p <crate>
  --lib --locked` alone for `ulo-ws`, with and without `tokio-server`, `ulo-graphql-ws`,
  `ulo-graphql-http` and `ulo-ws-redis`, and `cargo check -p <crate> --all-targets --locked` alone
  for those and `ulo-rpc`, `ulo-tokio` and `ulo-http-conformance`: each exit 0.
- `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --lib -p <crate>` for `ulo`, `ulo-ws` (with and
  without `tokio-server`), `ulo-graphql-ws`, `ulo-graphql-http`, `ulo-rpc`, `ulo-tokio`, `ulo-http`,
  the five adapters and `ulo-http-conformance`: each exits 0.
- `cargo +1.98.1 clippy` over `ulo`, `ulo-ws`, `ulo-graphql-ws`, `ulo-graphql-http`, `ulo-rpc`,
  `ulo-tokio`, `ulo-http`, `ulo-http-hyper`, `ulo-codegen-tests`, `ulo-http-conformance` and the
  five adapters with `--all-targets --no-deps --locked`: exit 0, 116 warning locations. One falls on
  a changed line: `type_complexity` on `mounted_parts`' return type
  (`crates/ulo/src/transport/server.rs:237`), whose shape, a `Result` of a handler list and an
  `Arc<dyn ..>`, is the one it had with `Timer` and `TimerMissing`. The checker reads
  `git diff -U0`'s hunks and the untracked files; run first with two real locations planted as
  changed, it reported them (`verify/checker-selftest.txt`). No macro's output changed;
  `cargo +1.98.1 clippy -p ulo-macro-lints --all-targets --no-deps --locked -- -D warnings`
  exited 0.
- **Containers:** the user's four, seaweedfs, mailpit, postgres:18 and redis:7, before and after
  every run and untouched. Each broker suite's containers were removed by the tests that started
  them. The Docker engine answered throughout.
