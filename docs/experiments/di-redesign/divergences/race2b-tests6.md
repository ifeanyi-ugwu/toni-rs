# Divergences: race 2b, tests batch 6, the conformance answers' build

This batch builds what the twenty-third response signed off: salvo's public `Closing<A>`
acceptor, actix's HTTP/2 drain declared in `EmbedLimits` and asserted on a build with actix-web's
`http2`, `Capabilities::durable_replies`, the Kafka late-topic case documented, `RpcClientModule`
closing its link (F327), and the Kafka link's consumer drops moved off the runtime (F328). Every
conformance suite passes three runs in a row. The Kafka suite takes 14 to 22 s where it took
100 s, its scenarios 1.7 to 14.3 s where they took 47 to 56 s.

Files added: `crates/ulo-http-actix/tests/{common/mod.rs, conformance_http2.rs}`. Files changed:
`.github/workflows/ci.yml`, `Cargo.lock`, `crates/ulo-http/src/embed.rs`,
`crates/ulo-http-salvo/{Cargo.toml, src/lib.rs, src/run.rs, tests/conformance.rs}`,
`crates/ulo-http-actix/{Cargo.toml, src/lib.rs, tests/conformance.rs}`,
`crates/ulo-http-conformance/src/cases/drain.rs`, `crates/ulo-rpc/src/{link.rs, client_module.rs}`,
`crates/ulo-rpc-conformance/src/{lib.rs, relay.rs, cases/app.rs, cases/delivery.rs,
cases/recovery.rs}`, `crates/ulo-rpc-kafka/{src/lib.rs, src/link.rs, tests/conformance.rs}`, and
F330 in the workspace's `FRAMEWORK_GAPS.md`. Neither `crates/ulo-rpc-nats/src/link.rs` nor
`crates/ulo-rpc/src/server.rs` is changed by this batch. The tree is `61dd2b54`, the NATS drain
fix, plus this batch.

## The signatures

```rust
// ulo_http_salvo: `run`'s third parameter was `acceptor: A`
pub struct Closing<A> { /* .. */ }                                              // new
impl<A: Acceptor> Closing<A> { pub fn new(handle: &Handle, acceptor: A) -> Self; }
impl<A: Acceptor + Send + 'static> salvo::conn::Acceptor for Closing<A> { /* .. */ }
pub async fn run<A: Acceptor + Send + 'static>(app: App<Bound>, handle: &Handle, server: salvo::Server<Closing<A>>,
    service: salvo::Service, signal: impl Future<Output = Signal> + Send) -> Result<Shutdown, BoxError>;

// ulo_http::embed
pub struct EmbedLimits { /* .. */ pub drain_http2: DrainHttp2 }                // new field
impl EmbedLimits { pub const fn drain_http2(self, drain_http2: DrainHttp2) -> Self; } // new
pub enum DrainHttp2 { GoAway, Reset }                                           // new; `NONE` declares `GoAway`

// ulo_rpc
pub struct Capabilities { /* .. */ pub durable_replies: bool }                 // new field; `new` sets `false`
impl Capabilities { pub const fn durable_replies(self, durable_replies: bool) -> Self; } // new

// ulo_rpc_conformance::relay
impl Relay {
    pub async fn cut(&self) -> usize;                       // was `-> ()`
    pub async fn cut_for(&self, outage: Duration) -> usize; // new
}
```

```toml
# ulo-http-actix
[features]
conformance-http2 = ["actix-web/http2"]   # new

[[test]]
name = "conformance_http2"
required-features = ["conformance-http2"]
```

A salvo caller builds `salvo::Server::new(ulo_http_salvo::Closing::new(&embedded, acceptor))`,
configures it as before, and passes the server to `run`. `RpcClientModule`'s signature is
unchanged; its binding gains an `on_destroy` hook. Callers writing `relay.cut().await;` compile
unchanged.

What each target declares:

| Target | `drain_http2` | `durable_replies` | Declared not applicable |
| --- | --- | --- | --- |
| hyper, axum, salvo, poem, rocket | `GoAway` | | as before |
| actix, `conformance` | `Reset` | | `drain_http2`, `drain_goaway`: the host listens with `listen`, which serves HTTP/1.1 alone |
| actix, `conformance_http2` | `Reset` | | none |
| Kafka | | `true` | none (was `recovery_after_disrupt`) |
| NATS, Redis, RabbitMQ, MQTT, TCP, UDP | | `false` | as before |

## Decisions

### 1. salvo's stop command follows a poll of the server that began after the drain did

- **Written:** `Closing<A>` holds the caller's acceptor and the handle's `Stopping`. Its `accept`
  races the inner accept against `stopping`, and once `stopping` resolves it drops the inner
  acceptor, closing the listener, and pends forever. `run` polls `try_serve` through
  `stop_after_close`, which polls `stopping` first and the server second, and calls
  `stop_graceful(drain)` after a server poll that began with `stopping` already resolved.
- **Why not batch 3's oneshot:** batch 3's private `Closing` signalled `run` through a oneshot
  once its acceptor was dropped. With the server built by the caller, `run` holds only
  `salvo::Server<Closing<A>>`, which has no accessor for its acceptor, so the receiver has no path
  to `run`. The ordering above needs no channel. A server poll that begins with `stopping`
  resolved reaches salvo's `select!` with no command queued, so the command branch is pending and
  the accept branch is polled, where `Closing::accept` sees `stopping` and drops the acceptor. The
  command is sent after that poll, and wakes the server through the receiver registered in it.
- **Probed against a violation:** with `stop` called before the server poll, `drain_http2` failed
  in 6 of 10 runs ("an HTTP/2 request during the drain ended after 4.0029s of a 4s drain
  window"), which is salvo's random `select!` reading the command first. With the ordering, the
  four drain scenarios passed 30 runs of 30.
- **Cost:** a `Closing` built from another embedding's handle keeps its listener open through this
  one's drain. `run`'s documentation says so; nothing detects it.
- **Also:** `Closing::new(&handle, acceptor)`, an associated function, rather than the free
  `closing(..)` batch 3's S2 sketched. salvo's `[dependencies]` no longer name `tokio`, which only
  the oneshot used.

### 2. `DrainHttp2` is a field, `GoAway` or `Reset`, and actix declares `Reset` in every build

- **Written:** `EmbedLimits::drain_http2: DrainHttp2`, beside `drain_pending` and
  `drain_abandoned`. `GoAway`: the host sends GOAWAY with `NO_ERROR` when its stop begins and closes
  the connection once its streams end. `Reset`: no GOAWAY; the connection keeps carrying requests
  to the draining app until the host's stop deadline, then is reset. The field's doc says what a
  `Reset` host means for a client, and that a host serving no HTTP/2 declares what it would do were
  it enabled.
- **Why every build:** whether actix-web's `http2` is on is a feature of another crate, which the
  adapter cannot read, and Cargo unification turns it on from anywhere in the application.
  actix's crate doc records the limit with that reason.
- **What the scenarios assert:** `drain_goaway` requires, where `GoAway`, a remote GOAWAY with
  `NO_ERROR` within the patience; where `Reset`, a connection that takes requests until it fails
  with no GOAWAY received, no sooner than the drain window and within the window plus the
  patience. `drain_http2` requires, where `Reset`, that the request sent during the drain is
  answered 503, because the held connection stays open and carries it; a failed request fails the
  scenario. Where `GoAway`, a refusal still passes, as before.

### 3. The h2c run is a second test target behind a test-only feature

- **Written:** `ulo-http-actix` gains `conformance-http2 = ["actix-web/http2"]` and a
  `conformance_http2` test target with `required-features`. The host moved to
  `tests/common/mod.rs` as `ActixHost<const H2C: bool>`: `ActixHost<true>` listens with
  `listen_auto_h2c`, `ActixHost<false>` with `listen`. Without the feature, `ActixHost<true>` is a
  const-evaluation error naming the flag.
- **Why a feature rather than `--features actix-web/http2` on the command line:** the host has to
  call `listen_auto_h2c`, which exists only with `http2`, and a test target can gate code on its
  own package's features alone.
- **Also:** the default target's not-applicable reason now names `listen`, not the build: under
  `--all-features` actix-web's `http2` is on there too, and the host still serves HTTP/1.1 alone.
  `Cargo.lock` gains actix-http's edge to `h2 0.3.27`, which CI's `--locked` check needs. CI's
  `test` job runs the h2c target after `cargo test --workspace`.

### 4. On a `durable_replies` link the recovery scenario requires the held call's answer, and the environment proves the cut

- **Written:** where `durable_replies`, the held call has to return its own value within its
  length plus `Budget::recovery`; anything else fails, naming the declaration. A `disrupt` that
  severs nothing would let that pass, so `Relay::cut_for(outage)` answers how many open
  connections it closed, and the Kafka broker's `disrupt` fails when that is zero. It also keeps
  the client's relay shut for `OUTAGE`, 4 s, closing every connection it accepts meanwhile.
- **The held call's length is now `settle + 3 s` on every link.** It was a fixed 3 s, and on
  Kafka, whose `settle` is 3 s, the reply was published about when `disrupt` ran. Now it is
  published 3 s after `disrupt` begins, inside the 4 s outage. On the other links the call still
  fails at the cut, so nothing waits on the longer length.
- **Probed against a violation:** NATS declaring `durable_replies(true)` failed with "the link
  declares `durable_replies`, so a call waiting on a severed connection is answered once the
  client reconnects, got: Err(RpcError { kind: Unavailable, message: \"the link closed before the
  reply arrived\", .. })".

### 5. The Kafka late-topic case is in the crate doc

- **Written:** the anchoring commit is accepted only while the group has no member, so a handler
  added in a deployment that rolls out while the group runs gets a topic with no committed offset.
  Each assignment of its partitions then starts at their end until a handler there settles a
  record and the next auto-commit records it, and a request produced before the first assignment,
  or during a rebalance, is skipped with the caller seeing its own `Timeout`. Stopping every
  instance before starting the new version anchors the topic.

### 6. `RpcClientModule` closes the link on destroy, and the suite closes its client apps

- **Written:** the binding is `m.singleton(..).on_destroy(..)`, the hook calling `link.close()`
  and logging a failure at `warn`. The module's doc says a call made after the hook connects again.
  The suite's `Client` gains `close`, which closes its `App<Connected>`; `Fixture::stop` calls it
  after stopping the server, and `two_instances` closes its caller.
- **Why the suite changed:** the client app was dropped, never closed, so no destroy hook would
  have run in any scenario.

### 7. The Kafka client's `close` waits for its reply consumer to be dropped

- **What forced it:** with decision 6 alone, every Kafka scenario hung. `unary_round_trip` alone
  had not finished after 300 s. A stack sample showed the reply router's task inside
  `BaseConsumer::drop`, in rdkafka's loop polling until the consumer reports closed
  (`rdkafka-0.39.0/src/consumer/base_consumer.rs:775`), with librdkafka's threads querying for a
  group coordinator. The hook's `close` only signalled the router and returned, the fixture then
  removed the broker, and the router's consumer, dropped after that, never closed.
- **Written:** `ClientSide` keeps the router's `JoinHandle`, and `close` signals the router and
  awaits it. The router drops the consumer before it ends, so the consumer leaves its group while
  the broker is there.
- **Checked:** `unary_round_trip` alone took 2.9 s.

### 8. F328: every consumer drop runs on a thread of its own

- **Measured** (temporary `eprintln!` timing, removed, `link.rs` restored and confirmed by
  `shasum -c`): with the broker up, each consumer drop took 100.8 to 114.5 ms, 64 drops in one suite
  run across the reply, group and control consumers; that is one 100 ms poll of rdkafka's close
  loop. With the broker gone, the drop had not returned after more than five minutes (decision 7).
  F328's other observation, a drop blocked in `rd_kafka_destroy` joining librdkafka's main thread
  with the broker up, did not reproduce: no drop exceeded 115 ms.
- **Written:** a private `Detached<T>` wraps the server's group and control consumers and moves the
  value's drop to a new thread, wherever the last `Arc` goes. The reply router awaits
  `drop_detached(replies)`, which answers once the drop has finished, so decision 7's ordering
  holds.
- **Why a thread rather than `spawn_blocking`:** a tokio runtime waits for its blocking tasks as it
  shuts down, so a consumer whose broker is gone would hold the runtime's shutdown without bound. A
  plain thread costs one spawn per consumer drop, and a thread that cannot be spawned drops the
  value where it is, logged at `warn`.

## Needs sign-off

### S1. salvo's ordering in place of a signal, and `Closing::new`

Decision 1. The alternative that keeps batch 3's oneshot is an extra `run` parameter carrying its
receiver, `closing(&handle, acceptor) -> (Closing<A>, Closed)`, which leaves `run` four
parameters and a pairing the types do not enforce.

### S2. `DrainHttp2 { GoAway, Reset }`, declared `Reset` by actix in every build

Decision 2. The alternative is a variant on an existing field, which would tie HTTP/2's drain to
`drain_pending` or `drain_abandoned`, both of which are HTTP/1.1 observations on actix.

### S3. A public, test-only `conformance-http2` feature and a CI step

Decision 3. Cargo features are public, so a user can enable it; it only turns on actix-web's
`http2`, which the user could enable directly. The alternative is a separate unpublished test crate
depending on `ulo-http-actix` and actix-web with `http2`.

### S4. `Relay::cut` answers a count, and the held call is `settle + 3 s`

Decision 4. The count is what lets a durable link's `disrupt` refuse to pass having severed
nothing.

### S5. The Kafka client's `close` now waits for its consumer to leave the group

Decisions 7 and 8. With the broker up that is about a tenth of a second, under the destroy hook's
bound. With the broker gone it is unbounded on the drop thread, and the hook's bound ends the wait.

### S6. Kafka's place in CI

The Makefile's comment gives the Kafka suite "578 s and 147 s on two runs" as the reason CI runs it
by hand only, and `ci.yml`'s broker job repeats it. The suite now takes 14 to 22 s. Left as written:
moving Kafka into the `rpc conformance (brokers)` job is a CI decision, and the comments change
with it.

### S7. DESIGN text these changes falsify, left for the fold

- transports §3.8: the salvo bullet's `run(app, &handle, server: salvo::Server<A>, ..)` and its
  `stop_graceful` description; the limits table, which lacks `drain_http2`, and its actix row.
- §5.2: the recovery scenario, which now branches on `durable_replies`, and `Broker::disrupt`'s
  duty on a durable link; the held call's length.
- §5.3: the `Capabilities` block lacks `durable_replies`; the Kafka row lacks it, the late-topic
  case and the detached consumer drops.
- §5.4: `RpcClientModule`'s `on_destroy` hook, and the Kafka client waiting for its consumer at
  close.

## Not covered

- **The durable outage is shown by construction, not observed.** The reply is published 3 s after
  `disrupt` begins and the relay stays shut for 4 s, so it is published while the client is out;
  no assertion reads when it was published or when librdkafka reconnected.
- **A server-side Kafka consumer is not awaited by `close`.** Its drop runs on its own thread when
  the last `Arc` goes. If the broker is gone by then, that thread blocks without bound; the runtime
  does not.
- **The TCP and UDP links' `close` leaves a client's connection open**, read from the source and
  filed as F330. The hook calls it; on those two links it does nothing for the client side.
- **actix serving HTTP/2 over TLS with ALPN.** The h2c run stands in for it; not run.

## Verification

Against known violations, each change temporarily undone or flipped, run, restored, and the files
confirmed byte-identical by `shasum -c`:

- salvo's stop sent before the server poll: `drain_http2` failed in 6 of 10 runs (decision 1).
- actix's h2c run declaring `GoAway`: `drain_goaway` failed in both modes, "the host declares
  `DrainHttp2::GoAway` and the connection failed otherwise than by its GOAWAY: connection closed
  because of a broken pipe".
- axum declaring `Reset`: `drain_goaway` failed in both modes ("sent GOAWAY after 599.084µs:
  declare `GoAway`"), and so did `drain_http2` ("the held connection stays open through the drain
  and carries the request to the app's 503, but the request failed").
- NATS declaring `durable_replies(true)`: `recovery_after_disrupt` failed (decision 4). That file
  then carried the parallel investigation's uncommitted edits, since committed as `61dd2b54`, so the declaration went in and out as one token by
  `sed`, the binary built in between, and `shasum -c` confirmed the file as it was found.

Three consecutive runs per target on stable, test time as libtest reports it; the hermetic targets
with `cargo test -p <crate> --test conformance`, the brokers with `--features integration`:

| Target | Run 1 | Run 2 | Run 3 | Result |
| --- | --- | --- | --- | --- |
| `ulo-http-hyper` | 0.32 s | 0.39 s | 0.33 s | 34 passed, 2 ignored |
| `ulo-http-axum` | 0.32 s | 0.37 s | 0.33 s | 36 passed |
| `ulo-http-salvo` | 0.33 s | 0.36 s | 0.40 s | 36 passed |
| `ulo-http-poem` | 0.75 s | 0.77 s | 0.78 s | 36 passed |
| `ulo-http-actix` | 1.05 s | 1.05 s | 1.07 s | 32 passed, 4 ignored |
| `ulo-http-actix`, `conformance_http2` with `--features conformance-http2` | 4.05 s | 4.07 s | 4.11 s | 36 passed |
| `ulo-http-rocket` | 4.05 s | 4.05 s | 4.19 s | 36 passed |
| `ulo-rpc-tcp` | 1.31 s | 1.32 s | 1.34 s | 20 passed |
| `ulo-rpc-udp` | 1.31 s | 1.32 s | 1.33 s | 19 passed, 1 ignored |
| `ulo-rpc-nats` | 3.20 s | 3.18 s | 3.28 s | 20 passed |
| `ulo-rpc-redis` | 4.99 s | 5.03 s | 5.03 s | 20 passed |
| `ulo-rpc-rabbitmq` | 30.51 s | 18.73 s | 20.48 s | 20 passed |
| `ulo-rpc-mqtt` | 6.83 s | 4.78 s | 4.88 s | 20 passed |
| `ulo-rpc-kafka` | 22.35 s | 20.50 s | 14.31 s | 20 passed |

actix's h2c run takes the drain window, 4 s, in `drain_goaway`, where a `Reset` connection ends at
actix's `shutdown_timeout`. The broker rows are a second set of three runs, on `61dd2b54` plus this
batch, with no other containers on the engine. A first set, run while the NATS investigation
looped its containers on the same engine, also passed fifteen of fifteen, and is discarded: that
investigation restarted the engine once during its runs, and the window is not known to exclude
them.

Kafka per scenario, from `--report-time` (libtest's, under `RUSTC_BOOTSTRAP=1` on the built test
binary), before this batch and on the final tree:

| | Before | After |
| --- | --- | --- |
| Suite | 102.41 s | 16.30 s |
| Fastest scenario | 47.26 s (`binary_payload`) | 1.66 s (`oversized_payload`) |
| Slowest scenario | 55.57 s (`drain`) | 14.33 s (`recovery_after_disrupt`) |
| `unary_round_trip` | 49.46 s | 2.16 s |
| `recovery_after_disrupt` | ignored | 14.33 s |

- `cargo check --workspace --all-targets --all-features` on stable, and `cargo +1.88 check
  --workspace --all-targets --exclude ulo-http-salvo --exclude ulo-graphql-async-graphql`, each
  print the same 17 warnings, all in `crates/ulo/src`, the set listed in `batch2a-cfgattr.md`.
  `cargo +1.88 check -p ulo-http-actix --all-targets --features conformance-http2` prints the same
  17. Locally, `--all-features` needs
  `CFLAGS=-I/opt/homebrew/opt/openssl/include LDFLAGS=-L/opt/homebrew/opt/openssl/lib`:
  `ulo-rpc-kafka`'s `tls` feature builds librdkafka against OpenSSL, and its configure script finds
  no `pkg-config` on this machine.
- `cargo test --workspace --no-fail-fast`: 350 passed, 0 failed, 62 ignored, 687 s wall time. The
  ignored count is batch 5's 61 plus `Closing`'s `ignore` doc example.
- `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --lib --all-features` over the seven touched
  library crates: no warning.

The first Kafka suite run after decision 6 hung on decision 7's bug, and the OrbStack engine stopped
answering `docker` commands for about 20 minutes, with host swap at 12.8 of 14 GB. After the hung run
was killed, the engine stayed unresponsive, and `orbctl stop` and `orbctl start` restored it. That
restart removed every running container, the parallel NATS investigation's included, which ran its
own `orb restart docker` during the same outage.
