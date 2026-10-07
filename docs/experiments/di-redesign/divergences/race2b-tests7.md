# Divergences: race 2b, tests batch 7, the conformance answers' second build

This batch builds what the twenty-fourth response signed off, with the user's addition: salvo's
stop ordered by a signal the embedding handle carries (S1), the `conformance-http2` doc line (S3),
Kafka in CI's broker job (S6) with the suite's concurrency bounded (F332), the TCP and UDP links'
`close` ending a client's connection (F330), the durable outage observed rather than constructed,
and the Kafka consumer drop threads documented. Every conformance suite passes three runs in a row,
Kafka five.

Files added: `crates/ulo-rpc-tcp/tests/client_close.rs`, `crates/ulo-rpc-udp/tests/client_close.rs`.
Files changed: `.github/workflows/ci.yml`, `Makefile`, `Cargo.lock`, `crates/ulo-http/src/embed.rs`,
`crates/ulo-http-salvo/{Cargo.toml, src/lib.rs, src/run.rs, tests/conformance.rs}`,
`crates/ulo-http-actix/{Cargo.toml, src/lib.rs}`,
`crates/ulo-rpc-conformance/src/{lib.rs, relay.rs, cases/app.rs, cases/recovery.rs}`,
`crates/ulo-rpc-kafka/{src/lib.rs, tests/conformance.rs}`,
`crates/ulo-rpc-tcp/{Cargo.toml, src/link.rs}`, `crates/ulo-rpc-udp/{Cargo.toml, src/link.rs}`, and
F330 and F332 in the workspace's `FRAMEWORK_GAPS.md`. The tree is `d4279b85` plus this batch.

## The signatures

```rust
// ulo_http::embed
impl<A: Embed> Handle<A> {
    pub fn listener(&self) -> HostListener;                       // new
    pub fn listeners_closed(&self) -> Option<ListenersClosed>;    // new; `None` when none was registered
}
pub struct HostListener { /* .. */ }                              // new; reports the listener closed on drop
pub struct ListenersClosed { /* .. */ }                           // new
impl Future for ListenersClosed { type Output = (); }

// ulo_rpc_conformance
pub trait Broker {
    const PARALLEL: Option<NonZeroUsize> = None;                  // new
    fn outage(&self) -> Option<relay::Outage> { None }            // new
    /* .. */
}
// ulo_rpc_conformance::relay
pub struct Outage { pub shut: Instant, pub reopened: Option<Instant> } // new
impl Relay { pub fn last_outage(&self) -> Option<Outage>; }      // new
```

`ulo_http_salvo::{Closing, run}` keep their signatures. `run` now answers `Err`, having closed the
app, when no `Closing` was built from its `handle`. `Tcp` and `Udp` keep theirs; their `close`
reaches the client side. A `Broker` implementation written before this batch compiles unchanged.

What each target declares:

| Target | `PARALLEL` | `outage` |
| --- | --- | --- |
| Kafka | `NonZeroUsize::new(4)` | the client relay's `last_outage()` |
| NATS, Redis, RabbitMQ, MQTT, TCP, UDP | `None` | `None` |

## Decisions

### 1. The handle counts registered listeners, and `run` stops salvo once every one has closed

- **Written:** `Shared` gains a `Listeners { registered, open, waiting }` behind a mutex.
  `Handle::listener` adds one to both counts and answers a `HostListener`, whose drop takes one
  from `open` and wakes the waiters at zero. `Closing::new` stores the `HostListener` beside the
  inner acceptor in one `Option<(A, HostListener)>`, so the `self.inner = None` in `accept`
  drops the acceptor, closing the socket, before the listener is reported closed. `run` takes
  `handle.listeners_closed()`, and its host future calls `stop_graceful(drain)` in the first poll
  that finds it resolved, then polls `try_serve`.
- **Why a count rather than one notice:** a handle may see more than one `Closing`, a server
  rebuilt or a value built and dropped before `run`. Counting open ones means `run` waits for
  the one inside its server whichever else exists, and a `Closing` dropped unused reports itself
  closed by its drop.
- **Why public rather than doc-hidden:** `Handle::stopping` and `Handle::host` are the adapter
  surface already, and an adapter for another host that keeps its listener until its connections
  end needs the same two calls. `embed.rs`'s module doc names them.
- **Poll order, probed:** with the closed signal polled before `try_serve`, and with `try_serve`
  polled first and the stop sent in the same poll the signal was seen, salvo's four drain
  scenarios passed 30 runs of 30 each (8 tests a run, both modes). The stop goes out as soon as
  the signal is seen, whether or not the server has been polled since.
- **Probed against a violation:** with `run` stopping salvo on `handle.stopping()` instead,
  polled first, `drain_http2` failed in 7 of 10 runs ("an HTTP/2 request during the drain ended
  after 4.0032215s of a 4s drain window, neither answered nor refused before then").

### 2. `run` refuses a server whose `Closing` came from another handle

- **Written:** when `handle.listeners_closed()` is `None`, `run` closes the app and answers
  `Err`: "the salvo server's `Closing` was built from another embedding's handle: build it with
  `Closing::new(&handle, acceptor)` from the handle passed to `run`".
- **Why refuse rather than warn:** `run`'s parameter type already requires a `Closing`, so none
  registered on `run`'s handle means one paired with another embedding, a deterministic mistake
  visible before serving. A warning would leave `run` with nothing to wait on. Probed: with the
  check removed and `listeners_closed()` resolving at once, the app shut itself down at startup
  with "transport `Http` failed: the salvo host server stopped before the shutdown began", the
  stop having been sent before any drain.
- **Test:** `run_refuses_a_closing_from_another_handle` in salvo's `tests/conformance.rs`, bounded
  by a 5 s timeout so a regression fails rather than hangs. Salvo's dev-dependency on `tokio`
  gains `time` for it.
- **Residual:** a second `Closing` built from `run`'s handle and kept outside the server holds the
  stop until it is dropped; the app's `close` is then bounded by the core's close bound. `run`'s
  doc says so.

### 3. `conformance-http2` is described as a suite feature in the manifest and the crate doc

- **Written:** the manifest comment opens "For the conformance suite; enables actix-web's
  `http2`. Not an application feature", and the crate doc gains a paragraph saying the feature
  changes nothing in the adapter and that an application serving HTTP/2 enables actix-web's
  `http2` itself.

### 4. Kafka's concurrency is a `Broker`-declared bound, four, with no retry

- **Written:** `Broker::PARALLEL: Option<NonZeroUsize>`, `None` by default. `conformance_suite!`
  emits one `static __ULO_RPC_CONFORMANCE_SLOTS: Slots` per invocation; each stamped test takes a
  slot before the scenario starts its environment and holds it until the scenario returns, its
  environment dropped. `Slots` is a `OnceLock<tokio::sync::Semaphore>` in a doc-hidden
  `__private` module; tokio's semaphore needs no runtime, so a permit released in one test's
  runtime wakes a waiter in another's. The Kafka broker declares 4.
- **Why a bound rather than a shared broker:** a container per scenario keeps every scenario's
  group offsets, control topic and reply topics apart without the suite having to namespace
  them, and the recovery scenario's relay cut cannot reach another scenario's client.
- **Why no retry:** a container exiting before its ready line is a symptom of the host running
  short; a retry would hide the cause and mask a container that fails for another reason. F332's
  failure was one container of twenty.
- **Observed:** `docker ps` sampled every 2 s during runs 2 to 5 counted at most 4 Kafka
  containers. Sampling can miss an overlap shorter than the interval.
- **Cost:** the suite takes 18 to 22 s, against 14 to 22 s with twenty at once in batch 6.

### 5. CI runs the Kafka suite in the broker job

- **Written:** a fifth step, `cargo test -p ulo-rpc-kafka --features integration --test conformance
  --locked`, after RabbitMQ's. The job's comment counts five `integration` targets and says the
  Kafka suite starts at most four broker containers at once; the Makefile's comment says CI runs
  it and keeps `conformance-kafka` for a local run.
- **Not run here:** the CI job itself. librdkafka already builds on the `test` job's runner, which
  compiles every workspace crate.

### 6. TCP: `close` advances a client epoch, and each client connection ends on it

- **Written:** `State` gains `client_epoch: watch::Sender<u64>` and `client_writers:
  Mutex<JoinSet<()>>`. `connect` subscribes to the epoch, spawns the writer into the set (reaping
  finished ones first), and races the reply lane's `read_frame` against the epoch changing.
  `write_frames` takes a `stop` future: on it the writer shuts its half, sending FIN, and returns;
  a server connection passes `pending()`. `close` advances the epoch, waits for the server's
  accept loop as before, then for every client writer. The reply lane ends, `route_replies` in
  `ulo-rpc` clears the calls waiting on it, and each fails `Unavailable` ("the link closed before
  the reply arrived"). A call made afterwards connects again on a new epoch subscription.
- **Changed behaviour:** the client writers belong to the link's `JoinSet`, so dropping the link
  aborts them; before, they were detached and ended when the queue's senders went. `RpcClient`
  holds its link for as long as it lives, so this changes nothing a client sees.

### 7. UDP: `close` ends each client socket's reply lane, and the lane owns the socket

- **Written:** what stands for a connection on UDP is the socket `connect` binds. The reply lane
  holds the only `Arc<UdpSocket>`; the send closure holds a `Weak` and fails "the UDP link's client
  socket is closed" once it cannot upgrade. The lane races `recv` against the same kind of epoch
  as TCP's, which `close` advances. When the lane ends the socket is released and the waiting
  calls fail `Unavailable`; a call made afterwards binds a new socket.
- **Also:** a reply lane ended by the OS reporting the server unreachable releases the socket
  too, where before the send closure kept it bound until the connection was replaced (read from
  the source).

### 8. F330's tests drive the destroy hook through a client app

- **Written:** `tests/client_close.rs` in each crate builds an app importing
  `RpcClientModule::for_root(link)` with a 30 s call timeout, starts a request to a raw peer that
  reads it and never answers, closes the app, and requires within 5 s: on TCP, the peer reading
  EOF and the call failing `Unavailable`; on UDP, the call failing `Unavailable` and the client's
  bound address, `0.0.0.0:<its port>`, binding again. Both crates gain `ulo-tokio` and
  `ulo-transport` as dev-dependencies.
- **Probed against a violation:** against the links as `d4279b85` has them, the TCP test failed
  with "the client's connection stayed open 5s after its app closed", and the UDP test with "the
  waiting call was still waiting 5s after its app closed". With the fix and an extra
  `std::mem::forget(Arc::clone(&socket))` in UDP's `connect`, the UDP test failed with "the
  client's socket on 0.0.0.0:58490 was still bound 5s after its app closed".

### 9. The durable outage is read from the relay, and the handler's answer moment stands for the publish

- **Written:** the relay keeps a `Window { until, shut, reopened }`. `cut_for` resets it, aborts
  and joins every relayed connection, and records `shut` once all have ended. `accept` records
  `reopened` at the first connection it relays after `shut`, which is the client's observed
  reconnection. `Relay::last_outage` answers both. `Broker::outage` reports it, the Kafka broker
  delegating to its client relay. The hold handler records `Instant::now()` in the probe as it
  returns its answer. On a `durable_replies` link the recovery scenario requires
  `outage.shut <= answered < reopened`, failing when the broker reports no outage or the relay
  relayed nothing after the cut.
- **What the assertion stands on:** the handler's answer moment, not the produce. The link
  publishes the reply after the handler returns, with at least 1 s to spare: the answer comes
  3.0 s after the cut and the relay lets no connection through before 4 s.
- **Probed against a violation:** with Kafka's `OUTAGE` at 1 s, so the client reconnected before
  the reply, `recovery_after_disrupt` failed with "the held call's answer is published while the
  client is out: published 3.009089834s after the cut, the client reconnecting 1.094179667s after
  it". Batch 6's scenario would pass the same violation, the call being answered either way (read
  from the scenario, not run).

### 10. The Kafka crate doc states where each consumer drop runs

- **Written:** a paragraph after the late-topic case. Each consumer is dropped on a thread of its
  own; the client's `close` waits for its reply consumer's drop, about a tenth of a second with
  the broker up; the server's group and control consumers are dropped with their last reference
  and not awaited, and once the broker is gone that drop blocks without bound, holding its thread
  and librdkafka's, not the tokio runtime's shutdown nor the process's exit.

## Needs sign-off

### S1. `Handle::listener` and `Handle::listeners_closed` as public adapter surface

Decision 1. The alternative is the same two calls `#[doc(hidden)]`, which leaves them callable
and undocumented for an adapter outside the workspace.

### S2. `run` refuses a mispaired `Closing`

Decision 2. The alternative, a `warn` and serving on, leaves `run` without a signal to wait on,
and so either stops salvo at once or never.

### S3. `Broker::PARALLEL`, Kafka at 4, no retry

Decision 4. The alternatives are one shared broker with a topic and group namespace per scenario,
or a retry of a container that exits during startup.

### S4. TCP's client writers owned by the link

Decision 6. The alternative keeps them detached and gives `close` abort handles only, which leaves
the writer's FIN unawaited when `close` returns.

### S5. The publish moment read off the handler

Decision 9. Observing the produce itself needs the server relay to recognise reply records in the
Kafka protocol stream.

### S6. DESIGN text these changes falsify, left for the fold

- transports §3.8: the salvo bullet's stop ordering, which is now the handle's listener signal and
  `run`'s refusal; the embedding handle's surface, which gains `listener` and `listeners_closed`.
- §5.2: `Broker::PARALLEL` and `Broker::outage`; the recovery scenario's outage assertion on a
  `durable_replies` link.
- §5.3: the Kafka row's drop behaviour (decision 10); CI running the Kafka suite.
- §5.4 and the TCP and UDP rows: `close` ends the client side.

## Not covered

- **No conformance scenario closes a client while a call waits.** F330's tests are per crate; the
  broker links' claim that `close` fails a waiting call `Unavailable` is asserted nowhere
  generically. A scenario in the suite would hold every link to it.
- **The produce moment.** Decision 9 asserts the handler's answer moment.
- **The CI job.** Not run; its step is the command the five local runs used.
- **Other containers on the engine.** `postgres:18` and `redis:7`, started outside this batch about
  five minutes before the first Kafka run, ran throughout the broker runs.

## Verification

Against known violations, each change temporarily undone or flipped, run, restored, and the files
confirmed byte-identical by `shasum -c`:

- salvo stopped on `handle.stopping()` rather than the listener signal: `drain_http2` failed in
  7 of 10 runs (decision 1).
- `listeners_closed()` answering `Some` with nothing registered: the refusal test failed (decision
  2).
- the TCP and UDP links as `d4279b85` has them: both `client_close` tests failed; UDP's socket kept
  alive past `close`: UDP's failed on the release check (decision 8).
- Kafka's outage shortened to 1 s: `recovery_after_disrupt` failed (decision 9).

Salvo's drain scenarios (`drain_http1`, `drain_http2`, `drain_goaway`, `drain_abandoned`, both
modes): 30 of 30 runs with the signal polled first, 30 of 30 with the server polled first.

Three consecutive runs per target on stable, test time as libtest reports it; the hermetic targets
with `cargo test -p <crate> --test conformance`, the brokers with `--features integration`, one
suite at a time:

| Target | Run 1 | Run 2 | Run 3 | Result |
| --- | --- | --- | --- | --- |
| `ulo-http-hyper` | 0.32 s | 0.31 s | 0.31 s | 34 passed, 2 ignored |
| `ulo-http-axum` | 0.32 s | 0.31 s | 0.31 s | 36 passed |
| `ulo-http-salvo` | 0.32 s | 0.31 s | 0.31 s | 37 passed |
| `ulo-http-poem` | 0.74 s | 0.73 s | 0.73 s | 36 passed |
| `ulo-http-actix` | 1.03 s | 1.02 s | 1.01 s | 32 passed, 4 ignored |
| `ulo-http-actix`, `conformance_http2` with `--features conformance-http2` | 4.04 s | 4.03 s | 4.02 s | 36 passed |
| `ulo-http-rocket` | 4.03 s | 4.01 s | 4.02 s | 36 passed |
| `ulo-rpc-tcp` | 1.31 s | 1.31 s | 1.31 s | 20 passed |
| `ulo-rpc-udp` | 1.31 s | 1.31 s | 1.31 s | 19 passed, 1 ignored |
| `ulo-rpc-nats` | 2.46 s | 4.61 s | 2.18 s | 20 passed |
| `ulo-rpc-redis` | 4.58 s | 4.41 s | 4.60 s | 20 passed |
| `ulo-rpc-mqtt` | 4.58 s | 4.66 s | 4.55 s | 20 passed |
| `ulo-rpc-rabbitmq` | 11.60 s | 12.38 s | 13.17 s | 20 passed |

salvo's count includes `run_refuses_a_closing_from_another_handle`.

Kafka, five consecutive runs of the built test binary: 22.13 s, 19.97 s, 21.47 s, 21.93 s, 17.98 s,
20 passed each; the first run took 24 s wall time.

- `tests/client_close.rs`: 1 passed on each of `ulo-rpc-tcp` and `ulo-rpc-udp`.
- `cargo check --workspace --all-targets --all-features` on stable (with the OpenSSL `CFLAGS` and
  `LDFLAGS` batch 6 records), and `cargo +1.88 check --workspace --all-targets --exclude
  ulo-http-salvo --exclude ulo-graphql-async-graphql`, each print the same 17 warnings, all in
  `crates/ulo/src`, the set listed in `batch2a-cfgattr.md`.
- `cargo test --workspace --no-fail-fast`: 353 passed, 0 failed, 62 ignored, 732 s wall time: batch 6's 350 plus salvo's refusal test and the two `client_close` tests.
- `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --lib --all-features` over the seven touched
  library crates: no warning.

Host swap stood at 13.9 to 14.2 GB of 15 GB through the broker runs, the OrbStack engine
answering throughout.
