# Divergences: race 2b, tests batch 13, the first poll's frame bound documented, port 0 for the TCP and UDP suites, every HTTP stream tracked

The thirtieth response, signed off 2026-10-07, settled three builds. HTTP's post-deadline cap
documents its one false positive: a body of 33 or more frames over 1 MiB, every frame ready, is one
the server waits on and answers 504. The TCP and UDP conformance suites and
`deadline_answers.rs` bind port 0 and hand the client the address the server reports, so no port is
probed and released (F343). The HTTP service wraps every response body of unknown length in
`Tracked` unless it already is, so `on_stream_end` fires for a stream however it was built (F344).

Files added: `crates/ulo-http/tests/stream_end.rs`, `crates/ulo-http/tests/common/mod.rs`, this
file.
Files changed: `crates/ulo-http/src/{limits.rs, service.rs, body.rs, sse.rs}`,
`crates/ulo-http/tests/post_deadline_body.rs`,
`crates/ulo-rpc-conformance/src/{lib.rs, relay.rs, cases/app.rs, cases/delivery.rs}`, the
`tests/conformance.rs` of `ulo-rpc-{tcp, udp, nats, redis, mqtt, rabbitmq, kafka}`,
`crates/ulo-rpc-tcp/tests/{conformance_cbor.rs, deadline_answers.rs}`, and F343, F344 and the new
F346 in the workspace's `FRAMEWORK_GAPS.md`. The tree is `3e7c2598` plus this batch.

## The signatures

```rust
// ulo_rpc_conformance::Broker
fn client_link(&self, server: &[BoundAddr]) -> Self::Link;   // was client_link(&self)

// ulo_rpc_conformance::relay::Relay (new)
pub async fn without_upstream() -> Relay;
pub fn forward_to(&self, upstream: SocketAddr);
```

No public item of `ulo-http` changed. Private to it: `HttpBody` gains the `tracked` mark, and
`marked` and `rewrapped` beside `tracked`.

## Decisions

### 1. The first poll's frame bound, in one sentence

- **Written:** beside the cap in `limits.rs` (`Timeout`) and in the service's doc of its
  post-deadline path (`expired`): "The first poll reads at most 32 frames, so a body of 33 or more
  frames over 1 MiB is one the server waits on and answers 504 even when every frame is ready."
- **Also:** step 6 of `AppService`'s doc said a body "past 1 MiB within" the grace renders 504,
  which batch 12 made untrue of a body ended on the first poll; it now reads "one the server waits
  on, past 1 MiB within it". See S3.

### 2. F343: the client's link is built from the addresses the servers bound

- **The shape that forced the change.** `Broker::link` and `Broker::client_link` were both called
  on the environment `start` returned, so a TCP or UDP environment had to name the server's address
  before the server bound it, which is why the suites probed a port and released it.
  `client_link` now takes `server: &[BoundAddr]`: every address the scenario's servers bound,
  `App<Bound>::addresses()` read right after `listen`, in the order they started, and empty on a
  broker link, whose servers bind nothing. The default still answers `link()`. See S1.
- **The harness:** `Server` keeps `addresses`; `client(broker, servers)` takes the servers it is
  built after. `Fixture::start` and `two_instances` already started their servers before the
  client; `two_instances` now builds its server list before the client rather than after.
- **TCP:** `link()` is `Tcp::new(127.0.0.1:0)`. The relay starts with no upstream,
  `Relay::without_upstream()`, and `client_link` aims it with `Relay::forward_to(bound.addr)`
  before answering `Tcp::new(relay.addr())`. A connection the relay accepts before then is closed
  at once, as a refused connect would fail it; naming a second, different upstream fails the
  scenario. See S2.
- **UDP:** `link()` is `Udp::new(127.0.0.1:0)`, and `client_link` answers `Udp::new(bound.addr)`.
  `start` holds nothing.
- **Both:** a `server` slice that is not exactly one address fails through `startup_failed!`. TCP
  and UDP declare `Addressed` delivery, so every scenario starts one server per client.
- **`deadline_answers.rs`:** the server's `Tcp` binds port 0 and the client's `Tcp` takes
  `server.addresses()`, failing the test unless there is exactly one.
- **No retry** was added anywhere.
- **The broker suites** take the parameter as `_server` and are otherwise unchanged.

### 3. F344: the service wraps every body of unknown length in `Tracked`

- **Written:** `respond` passes the response body through `HttpBody::tracked` with the request's
  execution, just inside `ExecBody`. `tracked` leaves a body of known length as it is, so
  `Content-Length` is never dropped, and wraps any other in `Tracked`. `ExecBody`'s drop still runs
  first, so a peer leaving cancels the execution with `Disconnected` before `Tracked` reads the
  reason.
- **Already wrapped:** `HttpBody` carries a private mark. `tracked` sets it on the body it builds
  and returns a marked body as it is; `Sse` marks the body it wraps itself; `TimedBody`, the
  service's wrapper for a deadline still pending after the answer, is built through `rewrapped`,
  which keeps the mark. A response built through `into_reply` is therefore wrapped once, by
  `into_reply`. A body rebuilt around a wrapped one through `HttpBody::new`, as a pre-dispatch entry
  can, starts unmarked and is wrapped again; both wrappers report, and the first report wins, so
  `on_stream_end` runs once. See S4.
- **The post-deadline path** is unchanged: its buffered copy has an exact length, so the service
  leaves it unwrapped, and it reports for the body it was read from.
- **`into_reply` still wraps.** Its doc promises it and nothing asked for its removal; on the
  normal path the service's wrap now makes it redundant. See S5.
- **`HEAD` answered by a `GET` handler.** The service strips the body before `respond` sees the
  response, so an untracked stream was dropped there with no report while one built through
  `into_reply` reported `CutOff`. `found` now passes the body through `tracked` before stripping it,
  and the dropped stream reports `CutOff` however it was built. The length a `HEAD` answer carries
  is unaffected: `tracked` leaves a known-length body as it is. Not asked for. See S6.
- **A 1xx, 204 or 304 answer** carrying a stream is wrapped like any other; the backend never
  writes the body, and dropping it reports `CutOff`, as it already did for one built through
  `into_reply`. Read from the code, not tested.

### 4. The tests

- **`stream_end.rs`, through `AppService::call`,** with the backend kept as a `Keeper` that
  decides whether a body is written to its end or dropped, moved with `written` into
  `tests/common/mod.rs`, which `post_deadline_body.rs` now shares. An error handler claiming a
  handler's error and an interceptor answering in the handler's place each answer
  `Response::new(HttpBody::stream(..))` of two chunks and register `on_stream_end`:
  `an_error_handlers_stream_reports_completed_once_written`,
  `an_interceptors_stream_reports_completed_once_written` (`Completed`, the body
  `first second`), `an_error_handlers_stream_dropped_before_its_end_reports_cut_off`,
  `an_interceptors_stream_dropped_before_its_end_reports_cut_off`
  (`CutOff(Some(Disconnected))`), and `a_head_answer_reports_its_unwritten_stream_cut_off` (an
  empty 200 and `CutOff`).
- **Why these two builders:** a handler's own return goes through `into_reply`, which already
  wrapped; an error handler and an interceptor answer `T::Reply` directly, bypassing it.
  A pre-dispatch entry answering a response is the third such path and is covered by the same line
  of the service, untested here.

## Unrelated gaps found

- **F346:** `crates/ulo-codegen-tests/tests/clients.rs:159-168` probes a port and releases it to
  find an endpoint down. It is the one probe-then-release port choice left in the workspace's tests
  (sweep below). Port 0 read back does not apply, since no server is meant to exist. A socket bound
  and never listening was tried as the replacement: on macOS a connect to one timed out
  (`ETIMEDOUT`) rather than being refused, so it is not one.

## Left for the transports DESIGN fold

- §2.6 (line 313): "On HTTP, the buffered copy of a body an error handler answers a passed deadline
  with reports for the body it was read from, wrapped in `Tracked` or not": the service now wraps
  every body of unknown length on every other path, which §2.6 does not say.
- §3's cancellation paragraph (line 495): "a stream an error handler hands over without
  `into_reply`, `Response::new(HttpBody::stream(..))`, carries no `Tracked` and reports on this path
  all the same" holds for the deadline path only; on the normal path the service wraps it.
- §5.2's conformance paragraph (line 874): `client_link(&self)` is now
  `client_link(&self, server: &[BoundAddr])`; `Relay` gains `without_upstream` and `forward_to`;
  "The TCP and UDP suites choose the server's port by binding a probe socket at port 0, reading its
  address and releasing it ... That interval is open" no longer holds; "the TCP and UDP probe
  sockets" leaves the list of startup paths failing through `startup_failed!`.

## Needs sign-off

### S1. `Broker::client_link` takes the servers' bound addresses

Decision 2: the trait change the brief allowed, chosen over a separate hook so the client's link is
a function of what the servers bound and no environment keeps interior state for it.

### S2. TCP's relay learns its upstream late

Decision 2: `Relay::without_upstream` and `forward_to`, with a connection accepted before the
upstream is named closed at once and a second, different upstream refused.

### S3. Step 6 of `AppService`'s doc corrected

Decision 1: a clause beyond the one sentence asked for, in the same doc.

### S4. A private mark rather than a second `Tracked` on every tracked body

Decision 3: without the mark, a body `into_reply` wrapped is wrapped again by the service and
reports once all the same; the mark saves the second layer on the common path.

### S5. `into_reply` keeps its wrap

Decision 3.

### S6. A `HEAD` answer's stripped stream reports `CutOff` however it was built

Decision 3: not asked for; the brief's "however it was built" read as covering the stripped body.

### S7. RabbitMQ passed three runs only with six test threads

Verification: at libtest's default parallelism every run failed scenarios on container startup
timeouts under swap pressure.

## Verification

Each new test against the change undone, `service.rs` restored by `shasum -c`; full output of
every failing run kept:

| Undone | Test | Result |
| --- | --- | --- |
| both of the service's `tracked` calls | the five of `stream_end.rs` | all fail: `left: None` against `right: Some(Completed)` (two), `Some(CutOff(Some(Disconnected)))` (two), and "the stream a `HEAD` answer dropped unwritten reported None, not `CutOff`" |
| `found`'s `tracked` before the strip | `a_head_answer_reports_its_unwritten_stream_cut_off` | fails with the same message; the other four pass |
| `forward_to` in the TCP suite's `client_link` | `unary_round_trip` | fails at the boot budget: "the server did not answer within the boot budget: Err(RpcError { kind: Unavailable, message: \"the link closed before the reply arrived\", .. }), every server still serving" |

No probe-then-release race was ever observed, so F343 has no failing run to reproduce; the passing
TCP runs reach the server only through the address it reported, the relay closing every connection
until `forward_to` names it.

Three consecutive runs per target on stable, one suite at a time, test time as libtest reports it;
the brokers with `--features integration`. No run logged a restart from `unshadowed`.

| Target | Run 1 | Run 2 | Run 3 | Result |
| --- | --- | --- | --- | --- |
| `ulo-http` stream_end | 0.00 s | 0.00 s | 0.00 s | 5 passed |
| `ulo-http` post_deadline_body | 0.16 s | 0.16 s | 0.16 s | 6 passed |
| `ulo-http-hyper` conformance | 0.32 s | 0.31 s | 0.31 s | 36 passed, 2 ignored |
| `ulo-http-hyper` route_timeout | 0.71 s | 0.70 s | 0.71 s | 6 passed |
| `ulo-http-axum` | 0.32 s | 0.31 s | 0.31 s | 38 passed |
| `ulo-http-actix` | 1.02 s | 1.02 s | 1.02 s | 34 passed, 4 ignored |
| `ulo-http-actix` conformance_http2 (`--features conformance-http2`) | 4.04 s | 4.04 s | 4.03 s | 38 passed |
| `ulo-http-poem` | 0.74 s | 0.73 s | 0.73 s | 38 passed |
| `ulo-http-rocket` | 4.11 s | 4.02 s | 4.01 s | 38 passed |
| `ulo-http-salvo` | 0.32 s | 0.31 s | 0.31 s | 39 passed |
| `ulo-rpc-tcp` conformance | 1.31 s | 1.31 s | 1.31 s | 24 passed |
| `ulo-rpc-tcp` conformance_cbor | 1.31 s | 1.31 s | 1.31 s | 24 passed |
| `ulo-rpc-tcp` deadline_answers | 0.21 s | 0.20 s | 0.20 s | 2 passed |
| `ulo-rpc-udp` conformance | 1.31 s | 1.31 s | 1.30 s | 23 passed, 1 ignored |
| `ulo-rpc-nats` | 7.33 s | 5.00 s | 4.10 s | 24 passed |
| `ulo-rpc-redis` | 8.54 s | 9.49 s | 10.18 s | 24 passed |
| `ulo-rpc-mqtt` | 8.63 s | 7.49 s | 8.07 s | 24 passed |
| `ulo-rpc-rabbitmq`, libtest's default threads | 152.37 s | 147.03 s | 137.93 s | 6, 18, 14 passed; 18, 6, 10 failed |
| `ulo-rpc-rabbitmq`, `-- --test-threads=6` | 118.91 s | 133.15 s | 180.78 s | 24 passed |
| `ulo-rpc-kafka` | 31.83 s | 39.14 s | 31.92 s | 24 passed |

**RabbitMQ at the default parallelism failed 34 scenarios over three runs, every one at the same
line:** `crates/ulo-rpc-rabbitmq/tests/conformance.rs:35`, "the RabbitMQ container did not start:
container is not ready: container startup timeout", testcontainers' wait for "Server startup
complete" running out while the suite started one container per scenario, 24 at once (it declares
no `PARALLEL`). That is before any link is built, so before anything this batch changed runs; the
suite's `client_link` only gains an ignored parameter. Swap stood at 15.8–16.1 GB of 17.4 GB
throughout, with the user's `postgres:18`, `redis:7`, `axllent/mailpit` and `chrislusf/seaweedfs`
running. Capping libtest at six threads, three runs passed 24 of 24. Batch 12's runs took 63–66 s
on the same suite; F341 holds the open question on this host's RabbitMQ timings, and a `PARALLEL`
bound for the RabbitMQ `Broker` is not added here. See S7. The failure files are under
`target/conformance-failures/`. The Docker engine answered throughout; no container was left
behind, and the user's containers were not touched.

- `cargo check --workspace --all-targets --all-features` on stable, with
  `CFLAGS=-I/opt/homebrew/opt/openssl/include LDFLAGS=-L/opt/homebrew/opt/openssl/lib`, and
  `cargo +1.88 check --workspace --all-targets --exclude ulo-http-salvo --exclude
  ulo-graphql-async-graphql`, each print the same 17 warnings, all in `crates/ulo/src`, the set
  listed in `batch2a-cfgattr.md`.
- `cargo +1.98.1 clippy -p ulo-macro-lints --all-targets --no-deps --locked -- -D warnings` passes.
- `cargo test --workspace --no-fail-fast`, with the OpenSSL flags: 422 passed, 0 failed, 64
  ignored: batch 12's 417 plus the five of `stream_end.rs`.
- `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --lib -p ulo-http -p ulo-rpc-conformance`: no
  warning. With `--document-private-items`, `ulo-http` fails on a redundant link target at
  `service.rs:48`, a line this batch did not touch.

### The port sweep

Every `TcpListener::bind`, `UdpSocket::bind` and `local_addr()` under `crates/`, `integration-tests/`
and `examples/`, read in place. In the workspace, the RPC relays and the HTTP suites' hosts bind
port 0 and keep their listeners; `client_close.rs` in `ulo-rpc-tcp` and `ulo-rpc-udp` binds a peer
and keeps it; no test names a fixed port. One probe-then-release remains:
`crates/ulo-codegen-tests/tests/clients.rs:161-164`, filed as F346. In `integration-tests`, which
the workspace excludes and nothing builds: `validated_transports.rs:81-84` and
`method_enhancers.rs:260-263` (`pick_free_port`), `bind_refusals.rs:64-66` (`free_port`), and the
fixed `GATEWAY_PORT` 19420 in `bind_phase_order.rs:22`; they are noted in F346. `examples/` binds no probe.
