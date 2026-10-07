# Divergences: race 2b, tests batch 8, the client-close scenario

F333: no RPC conformance scenario closed a client while a call waited, so only TCP and UDP were
held to transports DESIGN §5.4's claim that a closed client fails its waiting calls
`Unavailable`. `ulo-rpc-conformance` gains a `client_close` scenario that every link runs. It
failed on two links, Redis and Kafka, and both are fixed. On each, the waiting call already failed
`Unavailable` and one client connection outlived the close. Every RPC conformance suite passes
three runs in a row, Kafka five.

Files added: `crates/ulo-rpc-conformance/src/cases/client_close.rs`.
Files changed: `Cargo.lock`, `crates/ulo-rpc-conformance/src/{lib.rs, relay.rs, cases/mod.rs,
cases/app.rs}`, `crates/ulo-rpc-redis/src/link.rs`, `crates/ulo-rpc-kafka/src/{lib.rs, link.rs}`,
`tests/conformance.rs` in each of `ulo-rpc-{tcp,nats,redis,mqtt,rabbitmq,kafka}`,
`crates/ulo-rpc-{tcp,udp}/{Cargo.toml, tests/client_close.rs}`, and F333 in the workspace's
`FRAMEWORK_GAPS.md`. The tree is `84bd5bcb` plus this batch.

## The signatures

```rust
// ulo_rpc_conformance
pub trait Broker {
    fn client_connections(&self) -> impl Future<Output = Option<usize>> + Send { async { None } } // new
    /* .. */
}
// ulo_rpc_conformance::relay
impl Relay { pub async fn open(&self) -> usize; }                                                 // new
// ulo_rpc_conformance::cases
pub mod client_close { pub async fn client_close<B: Broker>(); }                                  // new
```

`conformance_suite!` stamps `client_close` between `recovery_after_disrupt` and `two_instances`.
A `Broker` implementation written before this batch compiles unchanged and runs the scenario
without its connection assertions. `Redis` and `Kafka` keep their public signatures.

What each target declares:

| Target | `client_connections` |
| --- | --- |
| TCP, NATS, Redis, MQTT, RabbitMQ | `Some(self.relay.open().await)` |
| Kafka | `Some(self.client.open().await)`, the client relay |
| UDP | `None` |

## What the scenario asserts

On a fixture's broker, server and client, the client's app importing
`RpcClientModule::for_root(link)`:

1. A call to `conformance.never`, with a 60 s timeout of its own, reaches the handler: the probe's
   `unanswered` count rises within `settle + 5 s`.
2. Where the environment counts the client's connections, it counts at least one, and the call is
   still waiting.
3. The client's app closes within 30 s and its report carries no failure.
4. The waiting call fails `Unavailable` within 5 s of the close returning.
5. Where the environment counts, the count reaches zero within 5 s and is still zero `settle`
   later, no call having been made.
6. A call from the same `RpcClient` is answered within the `boot` budget, each attempt bounded at
   500 ms. Where the environment counts, a connection is then open.

## Links that failed, and the fixes

TCP, UDP, NATS, MQTT and RabbitMQ passed on their first run.

### Redis: the client's publisher connection outlived the close

- **Failure:** "the client's connections were still open 5s after its app closed: Some(1)". The
  waiting call had failed `Unavailable`.
- **Cause:** the client side opens two connections: the Pub/Sub one, held by the reply lane's
  task, and a `ConnectionManager` for `PUBLISH`, held by `ClientSide`. `close` aborted the lane's
  task, which closed the first connection and ended the reply lane. `ClientSide` lives in the send
  closure of the `Outbound`, and `RpcClient` keeps that connection in its slot, marked closed, until
  its next call replaces it. The publisher's connection lived as long as that.
- **Fix:** `ClientSide::publisher` is a `Mutex<Option<ConnectionManager>>`, and `State::client`
  keeps the `Arc<ClientSide>` beside the lane's abort handle. `close` takes the publisher out,
  closing its connection, and a send on a closed client side fails "the Redis link's client is
  closed". A call after the close connects through a new `ClientSide`, as before.

### Kafka: the client's producer outlived the close

- **Failure:** the same message, `Some(1)`, with the call failed `Unavailable`.
- **Cause:** the client's `FutureProducer` sits in `ClientSide`, held through the send closure as
  on Redis. `ClientSide::close` stopped the reply router and awaited the reply consumer's drop,
  and left the producer.
- **Fix:** `ClientSide::producer` is a `Mutex<Option<FutureProducer>>`. `close` takes it after the
  router has ended and drops it with `drop_detached`, awaited: rdkafka's `BaseProducer` drop
  purges, flushes for up to 500 ms and destroys the handle, which closes its connections. A send
  clones the producer out of the slot, or fails "the Kafka link's client is closed".

### Kafka: an unclosed client blocked a test thread for about 45 s

- **Failure:** not an assertion. With the producer fix, the suite passed in 53.78 s against
  18–22 s in batch 7. `client_close` alone took 50.64 s, its body returning at 5.35 s (timed by
  temporary `eprintln!`s, removed, the file restored and checked identical with `cmp`).
- **Cause:** the post-close call (step 6) opens a connection that nothing closes, its app having
  closed. When the test's runtime shuts down, the container is already gone (the drop order read
  from the scenario; the 45 s matches F327's measurement). The reply router's
  future is dropped mid-`recv`, and its `StreamConsumer` with it, inline on the test thread, which
  blocks in librdkafka's destroy: F327's symptom, reached by a client dropped unclosed. The crate
  doc said the link drops each consumer on a thread of its own, which did not hold for this path.
- **Fix:** the router holds its consumer in `Detached`, whose drop runs on its own thread
  wherever the router's future is dropped. Its normal end takes the value out with a new
  `Detached::into_inner` and awaits `drop_detached` as before. `client_close` alone: 5.45 s.

## Decisions

### 1. A handler that never answers, ending at the drain

- **Written:** `conformance.never` on `CoreController` counts its entry in the probe's
  `unanswered` and returns when its execution is cancelled or its server drains.
- **Why not `conformance.stall`:** on UDP no `cancel` reaches the server: the waiting call's
  `cancel` goes out over the client socket the close released, and fails. `stall` then held the
  server's stop for the 5 s drain window, and the UDP suite took 5.03 s, against 1.31 s with
  `never`. TCP's server cancels the call when the client's connection ends. The brokers were not
  run with `stall`.
- **Why the entry count:** step 1 shows the call at its handler before the close, which a sleep
  would only assume.

### 2. The connection count is a defaulted `Broker` method read from the relay

- **Written:** `Broker::client_connections`, `None` by default. Each environment with a client
  relay answers `Relay::open`: the relayed connections whose two directions have not both ended,
  finished tasks reaped first.
- **Why on `Broker`:** the scenario is generic over `B`, and the relay is a private field of each
  environment. UDP carries no connection to count and keeps the default.
- **What it cannot tell:** a connection ended by a reset counts as ended, the same as one ended by
  a FIN.

### 3. The client's close must report no failure

- **Written:** `Client::closed` answers `App::close`'s `Result<Shutdown, ShutdownError>`. The
  scenario fails on `Err`, and the per-crate tests do the same. `Client::close` still discards
  the report, for the other scenarios' teardown.
- **Why:** a destroy hook that outlasts `hook_timeout`, 10 s by default, is dropped by the core,
  and dropping its future can end the link's tasks itself. With TCP's `close` not advancing the
  client epoch, the hook hung on the client writers until that bound. Dropping it dropped the
  writers' `JoinSet`, which aborted them, and the peer read EOF. The trimmed TCP test passed
  against that close in 10.01 s until it required a clean report. The core records the drop as a
  `ShutdownFailure`, so requiring `Ok` fails a close that ended only at its bound.

### 4. The count stays at zero for `settle`

- **Written:** after the count reaches zero, the scenario sleeps the environment's `settle` and
  reads it once more.
- **Why:** a link that reconnects behind a closed client reaches zero and then opens a connection
  again with no call made.

### 5. A call after the close connects again

- **Written:** step 6, retried until answered or the `boot` budget passes.
- **Why:** §5.4 says a call made after the hook connects again, and a fix to `close` that left
  the link unusable would otherwise pass. The retry covers a broker that routes replies to a new
  connection late: a Kafka reply group takes its partitions seconds after it connects. Only an
  answer passes.

### 6. The per-crate `client_close` tests are trimmed, not deleted

- **Written:** each keeps what the scenario does not assert, plus the clean report (decision 3).
  The `Unavailable` assertion is removed from both, the scenario making it on every link.
  - TCP, `closing_the_client_app_ends_its_connection_cleanly`: a raw peer reads EOF, with nothing
    written after the call's frame. The relay cannot tell a FIN from a reset (decision 2), and a
    `cancel` sent ahead of the FIN would show here as a byte read.
  - UDP, `closing_the_client_app_releases_its_socket`: the client's address, `0.0.0.0:<its
    port>`, binds again. A UDP environment has nothing for `client_connections` to count.
- **Also:** both crates drop the `ulo-transport` dev-dependency, which only the removed assertion
  used. `Cargo.lock` loses two lines. Each test's module doc names what the scenario covers.

## Needs sign-off

### S1. The post-close reconnect assertion

Decision 5, beyond F333's candidate fix. The alternative leaves §5.4's second half to prose.

### S2. A clean close report required

Decision 3. The alternative bounds the close with a timer shorter than `hook_timeout`, which ties
the scenario to the core's default.

### S3. The per-crate tests trimmed

Decision 6. The alternatives keep both tests whole, duplicating the `Unavailable` assertion, or
delete them, losing the clean-EOF and socket-release assertions.

### S4. `Broker::client_connections` as suite surface

Decision 2.

### S5. Kafka's client `close` awaits the producer's drop

The Kafka fix. The drop flushes for up to 500 ms when records are queued. The alternative drops it
detached without awaiting, which returns from `close` before the connections have closed.

### S6. DESIGN text these changes falsify, left for the fold

- transports §5.2, the conformance paragraph: `Broker::client_connections` and `Relay::open`; the
  scenario list gains the client close.
- §5.3, the shutdown paragraph: Redis's client `close` releases its publisher; Kafka's client
  `close` drops the producer after the reply router, and the router's consumer is dropped on its
  own thread when the router is dropped. The paragraph's last sentence, on TCP and UDP, is batch 7's
  S6.
- §5.3, the Kafka row: the client's `close` awaits the producer's drop as well as the router's.
- §5.4: "On Kafka the hook's `close` waits for the reply router to drop its consumer" gains the
  producer.

## Not covered

- **UDP's client side in the scenario.** No relay carries it. The per-crate test keeps the
  socket-release check.
- **A server's connections at its close.** No scenario asserts that `Link::close` on a server
  ends its connections. Not probed.
- **A short reconnect between samples.** Decision 4 reads the count at two moments. A link that
  opens and closes a connection between them passes.
- **Other containers on the engine.** `postgres:18` and `redis:7`, started outside this batch, ran
  throughout the broker runs.

## Verification

Against known violations, each change temporarily made, run, and the file restored and confirmed
byte-identical by `shasum -c`:

| Violation | Run | Failure |
| --- | --- | --- |
| Redis and Kafka links as `84bd5bcb` has them | each broker suite | `client_close`: "the client's connections were still open 5s after its app closed: Some(1)" |
| NATS `close` returns at once on a link with no server side | NATS suite | `client_close` alone: "the call waiting when its client closed did not finish within 5s"; 20 passed |
| TCP `connect` refused once the client epoch has advanced | TCP suite | `client_close` alone: "a call after the client's close was not answered within the boot budget: Err(RpcError { kind: Unavailable, message: \"the tcp link could not connect: closed for good\", .. })" |
| TCP `close` opens a connection to the server 100 ms after returning, held 10 s | TCP suite | `client_close` alone: "the closed client opened a connection with no call made", `left: Some(1)` |
| TCP `close` without its client-epoch advance | TCP suite | `client_close` alone: "the client app's close reported a failure: shutdown on `conformance` finished with 1 failure" |
| the same | TCP `client_close` test | the same message, after 10.01 s |
| UDP `close` without its client-epoch advance | UDP suite | `client_close` alone: "the call waiting when its client closed did not finish within 5s" |
| the same | UDP `client_close` test | "the client's socket on 0.0.0.0:58283 was still bound 5s after its app closed" |

The Redis and Kafka rows are the first runs of the scenario, before the fixes.

Three consecutive runs per target on stable, test time as libtest reports it, one suite at a time;
the brokers with `--features integration`:

| Target | Run 1 | Run 2 | Run 3 | Result |
| --- | --- | --- | --- | --- |
| `ulo-rpc-tcp` | 1.32 s | 1.31 s | 1.31 s | 21 passed |
| `ulo-rpc-udp` | 1.31 s | 1.31 s | 1.30 s | 20 passed, 1 ignored |
| `ulo-rpc-nats` | 2.85 s | 2.49 s | 2.76 s | 21 passed |
| `ulo-rpc-redis` | 4.96 s | 5.03 s | 4.95 s | 21 passed |
| `ulo-rpc-mqtt` | 5.10 s | 4.65 s | 4.98 s | 21 passed |
| `ulo-rpc-rabbitmq` | 18.07 s | 19.82 s | 14.04 s | 21 passed |

Kafka, five consecutive runs: 22.83 s, 20.98 s, 24.18 s, 25.91 s, 23.80 s of test time, 21 passed
each; 32, 21, 25, 26 and 24 s wall time.

RabbitMQ's 14–20 s is above batch 7's 11.6–13.2 s. `client_close` alone took 2.79 s on RabbitMQ,
and a suite's time is that of its slowest scenario run in parallel; the rise was not traced. Swap stood at 13.0 to 13.8 GB of 14 GB through the broker
runs. `client_close` alone on the other brokers: NATS 0.96 s, Redis 0.94 s, MQTT 0.91 s.

- `tests/client_close.rs`: 1 passed on each of `ulo-rpc-tcp` and `ulo-rpc-udp`.
- `cargo check --workspace --all-targets --all-features` on stable, with
  `CFLAGS=-I/opt/homebrew/opt/openssl/include LDFLAGS=-L/opt/homebrew/opt/openssl/lib`, and
  `cargo +1.88 check --workspace --all-targets --exclude ulo-http-salvo --exclude
  ulo-graphql-async-graphql`, each print the same 17 warnings, all in `crates/ulo/src`, the set
  listed in `batch2a-cfgattr.md`.
- `cargo test --workspace --no-fail-fast`: 355 passed, 0 failed, 62 ignored, 548 s wall time:
  batch 7's 353 plus `client_close` in the TCP and UDP conformance suites.
- `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --lib --all-features` over
  `ulo-rpc-conformance`, `ulo-rpc-redis` and `ulo-rpc-kafka`, with the OpenSSL flags: no warning.
  `cargo check -p ulo-rpc-kafka --features integration --tests` after the harness's last doc edit,
  made once its five runs had finished: no warning.

The OrbStack engine answered throughout.
