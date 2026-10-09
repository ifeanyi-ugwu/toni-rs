# Divergences: race 2b, tests batch 19, WebSocket and gRPC report the stream they write (S1, S2, S3), the stream a link cannot carry reports nothing (S5), one refusal phrase (S6), a bounded `max_inflight` default with holding links pausing (S9), Kafka's blocking calls on its own threads (S10), MQTT's refusal documented, and F364 found and fixed

The thirty-fifth response and its addendum, signed off 2026-10-09, settled S1–S10 of batch 18.
WebSocket's `Reply::Many` holds a plain stream, tracked where `message` writes it (S1, F358).
gRPC tracks a marked reply at the dispatcher's outermost body, its trailers deciding the outcome
(S2, F359). The core's `on_stream_end` doc states the rule with no transport exception (S3). On
RPC a stream the link cannot carry is never tracked (S5). Every refusal of a call because the
server is draining or closing reads "the server is shutting down" (S6). `Count::Default` means
1,024 in-flight calls on every transport's `max_inflight`; AMQP and Kafka stop taking requests at
the bound and the rest refuse `unavailable` with `RetryAfter` (S9). Kafka's blocking calls run on
threads of its own and the anchor's backoff waits on the app's timer (S10). The MQTT crate doc says
why the link refuses rather than holds work, citing the Mosquitto probe. F364 recurred under disk
load and its cause, a client race, is fixed, with the TCP gap that set it up (F366).

Files changed: `crates/ulo/src/execution/mod.rs` (doc only); `crates/ulo-transport/src/{count.rs,
error.rs}`; `crates/ulo-http/src/{backend.rs, server.rs, config.rs}`,
`crates/ulo-http/tests/max_inflight.rs` (new); `crates/ulo-ws/src/{transport.rs, __private.rs,
connection.rs, gateway.rs, module.rs, server.rs}`, `crates/ulo-ws/tests/stream_end.rs` (new);
`crates/ulo-grpc/src/{dispatch.rs, server.rs}`; `crates/ulo-codegen-tests/proto/probe.proto`,
`crates/ulo-codegen-tests/tests/stream_end.rs` (new); `crates/ulo-rpc/src/{client.rs,
dispatch.rs, link.rs, server.rs}`, `crates/ulo-rpc/tests/stream_end.rs`;
`crates/ulo-rpc-conformance/src/{lib.rs, cases/mod.rs, cases/app.rs, cases/admission.rs (new)}`;
`crates/ulo-rpc-tcp/src/link.rs`, `crates/ulo-rpc-tcp/tests/late_goaway.rs` (new);
`crates/ulo-rpc-udp/src/link.rs`, `crates/ulo-rpc-udp/tests/close_read.rs`;
`crates/ulo-rpc-redis/src/link.rs`; `crates/ulo-rpc-mqtt/src/{lib.rs, link.rs}` (MQTT's
`lib.rs` doc only); `crates/ulo-rpc-rabbitmq/src/{lib.rs, link.rs}`;
`crates/ulo-rpc-kafka/{Cargo.toml, src/lib.rs, src/link.rs}`; `Cargo.lock`; F355, F358, F359 and
F364 appended and F366 filed in the workspace's `FRAMEWORK_GAPS.md`. The tree is `20e98a30` plus
this batch.

## The signatures

```rust
// ulo_transport::Count (new associated items)
impl Count {
    pub const DEFAULT_MAX_INFLIGHT: u32 = 1024;
    pub fn max_inflight(self) -> Option<usize>;   // Default: Some(1024), Max(n): Some(n), Unlimited: None
}

// ulo_ws::Reply (variant's field changed)
pub enum Reply {
    None,
    One(Frame),
    Many(BoxStream<'static, Result<Frame, BoxError>>), // was Tracked<BoxStream<..>>
}
```

A hand-built WebSocket stream reply is `Reply::Many(Box::pin(stream))`. Every other public item
is unchanged in shape. What changes in meaning: `Count::Default` on `ulo_http::Server::max_inflight`
and `HttpConfig::max_inflight`, `ulo_rpc::Server::max_inflight`, `ulo_grpc::Server::max_inflight`,
and `ulo_ws::Server`, `WsModule` and gateway `max_inflight` now bounds at 1,024; the docs of each
say so. `ulo_rpc_kafka::Kafka` implements `Link::max_inflight`. `ulo-rpc-kafka` gains
`ulo-transport` as a dependency, already in the workspace; `Cargo.lock` gains that one edge.

Private: in `ulo-rpc`, `Conn::register`, `Conn::ended` and `ClientInner::register`, and
`Call`'s `_permit` field renamed `permit`; in `ulo-grpc`, `Streamed` and `CallBody::{tracking,
report}`; in `ulo-rpc-udp`, `refuse` without its `message` parameter; in `ulo-rpc-rabbitmq`,
`DEFAULT_PREFETCH` renamed `UNLIMITED_PREFETCH`; in `ulo-rpc-kafka`, `Kafka::{max_inflight,
timer}`, `ServerSide::{inflight, limit}`, `at_bound`, `pause`, `InFlight` and `blocking`, and
`coordinator_retried` async and generic over the attempt's future. In `ulo-rpc-conformance`, the
public `cases::admission::over_the_bound` scenario, stamped by `conformance_suite!` as
`over_the_bound`, and the crate-private `BOUNDED`, `BOUNDED_TOO`, `server_bounded`,
`Fixture::start_bounded` and `Probe::bounded_peak`.

## Decisions

### 1. F364 first: the hunt, the cause and two fixes

- **The hunt, before any edit.** The three TCP suites were built once from `20e98a30` and run
  from copies of their binaries (`batch19/f364/bin/`), so no edit and no `cargo clean` reached
  them; every run's full output is in `batch19/f364/runs/`, one line per run in `summary.txt`.
  - 180 runs under 14 `yes` processes on the 10-core host: all passed.
  - 120 runs under 4 `yes` processes, from 07:23 to 07:26, while by every sign the coordinator's
    `cargo clean` was removing 284 GiB of build output; it finished at 07:30, and its start was
    not recorded. Two failed at `drain.rs:135:18`, `run-b-02-conformance_tls.txt` and
    `run-b-40-conformance_cbor.txt`, each with `Err(RpcError { kind: Timeout, message: "the call
    timed out", details: Details([]) })` after the scenario's one-second wait. Neither shows a
    build or missing-file error.
  - 120 runs during the full rebuild after the clean: all passed.
- **The cause, by reading.** `RpcClient::open` took a connection `ClientInner::connection` had
  found open, then inserted its call into the connection's `pending` map
  (`crates/ulo-rpc/src/client.rs:270-273` on `20e98a30`). The reply lane's task ends when the
  server closes the connection, setting `open` to false and clearing `pending` (`client.rs:343`).
  Between the two steps the call was registered on a lane that had already failed every call it
  held, its frame queued to a dead connection, and it waited out its own timeout.
- **The probe.** A temporary 30 ms sleep between the two steps (`batch19/runs/f364-probe/`) made
  `drain_window` fail 15 of 15 runs, the first call sent during the drain timing out. With the fix
  and the same sleep, 15 of 15 passed, twice (`f364-probe/widened-fixed-*`, `f364-probe2/`). The
  probe was removed and the file checked by hash.
- **The fix.** `Conn::register` checks `open` under the lock `Conn::ended` clears `pending` under,
  so a registered call is answered by the lane or failed by its end. `ClientInner::register`
  replaces a connection whose lane ended in between with a new one, once, and fails the call
  `Unavailable` if the second ends as fast. A `goaway` still sets `open` without the lock: a call
  registered as it arrives goes to a draining server, which refuses it. See S12.
- **The precondition, F366.** For the after-close call to reuse a connection at all, the
  connection must have missed the drain's `goaway`; that step is read, not observed. `Tcp::drain`
  sends `goaway` to the connections in `State::connections`, and a TLS connection registers only
  after its handshake, so one accepted before the drain and finishing its handshake after it got
  none. `serve` now sends its own `goaway` when it registers into a draining link; the drain
  advances the phase before it reads the map and a connection reads the phase after it registers,
  under that map's lock, so one of the two reaches every connection
  (`crates/ulo-rpc-tcp/src/link.rs`, `serve`). Filed as F366 and fixed in this batch.
- **After the fix:** the three suites, rebuilt from the fixed tree and run from copies, passed 120
  of 120 runs under 4 `yes` processes (`batch19/f364/runs-fixed/`). Those binaries predate the
  `over_the_bound` scenario and run 25 scenarios each.

### 2. S1: WebSocket tracks the stream where it writes it

- `ulo_ws::Reply::Many` holds a plain stream (`crates/ulo-ws/src/transport.rs`);
  `Answer::<ViaStream>::answer` builds it bare (`__private.rs`); `message` wraps it in `Tracked`
  in its `Ok(Reply::Many(stream))` arm, where it hands it to `pump` (`connection.rs`), with a
  comment saying a discarded stream never reaches that arm. `Reply`'s doc states the rule.
- **No post-deadline path:** a WebSocket message has no deadline, so the exception RPC and HTTP
  keep for a stream the error handlers answer after one has nothing to apply to here.
- The change is confined to the reply path: `connection.rs`'s `message` arm and `pump`'s
  parameter, `transport.rs`'s `Reply`, and the one constructor. `ulo-graphql-ws` builds no
  `Reply::Many` and is unchanged.

### 3. S2: gRPC tracks a marked reply at the dispatcher, its trailers deciding

- **The mark:** `encode_stream` builds its body over the bare items and inserts a private
  `Streamed` in the reply's extensions (`crates/ulo-grpc/src/dispatch.rs`). `LateItems::failed`
  no longer reports `CutOff(None)` itself; the non-zero trailers it ends with do.
- **The report:** `Dispatcher::call` reads the mark off the final response and hands it to
  `CallBody`, which reports once, through `CallBody::report`:
  - `Completed` as it yields trailers carrying `grpc-status: 0`;
  - `CutOff(exec.cancel_reason())` as it yields trailers with any other status, an `Err` item's
    included, as the body ends without trailers or yields an error, as `end_with` writes the
    deadline's DEADLINE_EXCEEDED or a panic's INTERNAL, or as it is dropped before any of these.
  `report` takes the `tracking` flag, so whichever comes first is the one report. `Drop` writes
  `ClientCancelled` before it reports, so a caller abandoning the reply is
  `CutOff(ClientCancelled)`. The panic arm's existing unconditional report is kept and precedes
  `end_with`'s.
- **Discarded:** a marked reply an interceptor drops is never handed to `CallBody` and reports
  nothing. An interceptor that answers `cx.reply(..)` answers an unmarked single message.
- **Post-deadline, checked:** before this batch the post-deadline path reported `CutOff(Deadline)`
  from the wrap inside the dropped body. `expired` now reads the mark before `single` and, when
  the reply it writes no longer carries it (dropped unwritten, or replaced by DEADLINE_EXCEEDED
  after a failed read), reports `CutOff(exec.cancel_reason())`, which is `Deadline` there. A
  single message the error handlers answered within the grace keeps the mark through `single`'s
  `Buffered` body and reports `Completed` at its trailers.
- **What a report now means:** `Completed` moves from the moment the item stream returned `None`
  to the moment the trailers are handed to hyper, which §2.6 names as gRPC's clean end.
- **Not tracked, as before:** a hand-built body, and the health and reflection services' replies.
- The module doc states the rule.

### 4. S3: the core states the rule with no exception

`Execution::on_stream_end` (`crates/ulo/src/execution/mod.rs`) keeps "it reports the reply
actually written; a stream discarded before the response is sent is not a reply and reports
nothing", and "The first report an execution receives is the one kept". The WebSocket and gRPC
exception is gone.

### 5. S5: RPC wraps after the capability check

`answer` hands `stream_reply` the bare stream; `stream_reply` checks the link's shapes first,
drops the stream and answers `err` of kind `internal` when the link carries no streamed reply, and
wraps the stream in `Tracked` only after the check (`crates/ulo-rpc/src/dispatch.rs`). The
post-deadline arm keeps its wrap-and-drop.

### 6. S6: one refusal phrase

Every refusal of a call because the server is draining or closing reads "the server is shutting
down". Changed:

| Site | Was |
| --- | --- |
| `crates/ulo-rpc/src/dispatch.rs`, `start`, `Execution::open` refused | "the server is draining" |
| `crates/ulo-rpc/src/dispatch.rs`, `unhandled`, `Execution::open` refused | "the server is draining" |
| `crates/ulo-rpc-tcp/src/link.rs`, `refuse` (after the inbound stream ended) | "the server is draining" |
| `crates/ulo-rpc-udp/src/link.rs`, `refuse` from the receive loop | "the server is draining" |
| `crates/ulo-rpc-udp/src/link.rs`, `refuse` from `final_read` at close | "the server has closed" |
| `crates/ulo-rpc-redis/src/link.rs`, `refuse` | "the server is draining" |
| `crates/ulo-rpc-mqtt/src/link.rs`, the event loop's refusal | "the server is draining" |
| `crates/ulo-transport/src/error.rs`, `CLOSED_MESSAGE`, a call whose resolution met the closed app | "the application is shutting down" |

UDP's `refuse` loses its `message` parameter, both callers now passing the same text. Already
using the phrase, unchanged: HTTP's `render::draining`, gRPC's refused `Execution::open`,
WebSocket's refused upgrade and refused message, GraphQL-WS's subscription at the drain. NATS,
RabbitMQ and Kafka write no refusal of their own. Left as they are: the core's
`Closed`'s `Display`, "the application is shutting down", a Rust error's text the transport
replaces on the wire; WebSocket's close reason "server shutting down" with code 1001, which ends a
connection rather than refusing a call (S13); and "the server is over its in-flight limit", a
different condition.

### 7. S9: a bounded default, read in one place

**Where `max_inflight` exists and what `Default` meant, before:**

| Transport | Setting | `Count::Default` before | Over the bound |
| --- | --- | --- | --- |
| HTTP | `ulo_http::Server::max_inflight`, `HttpConfig::max_inflight`, the embedding's | no bound | 503, `Retry-After`, "the server is at its limit of requests in flight", before routing (`crates/ulo-http/src/render.rs`, `shed`) |
| RPC | `ulo_rpc::Server::max_inflight`, handed to `Link::max_inflight` | no bound; AMQP's prefetch 64 per pattern consumer | `err` `unavailable` with `RetryAfter`; an event left unacknowledged |
| gRPC | `ulo_grpc::Server::max_inflight` (`max_per_connection` beside it) | no bound | UNAVAILABLE |
| WebSocket | `ulo_ws::Server`, `WsModule` and gateway `max_inflight`, per connection | 64 messages per connection | the connection stops reading |

**The one place:** `Count::DEFAULT_MAX_INFLIGHT` and `Count::max_inflight()` in
`crates/ulo-transport/src/count.rs`. HTTP's `HttpConfig::inflight_limit`, RPC's `prepare`, gRPC's
`prepare`, WebSocket's `Limits::resolve` (`Count::Default.max_inflight()` as the built-in default
under the server's and module's), RabbitMQ's and Kafka's `Link::max_inflight` read it. gRPC's
`max_per_connection` keeps no bound of its own at `Default`, the server's bound applying.

**HTTP over the limit**, read and kept: `AppService::respond` calls `Admission::try_admit` before
anything else and answers `render::shed`, 503 with `Retry-After` from `shed_retry_after`, one
second unset, and `Routing::Unrouted`.

**Links that hold work stop pulling:**
- **RabbitMQ:** the prefetch is the server's bound, capped at 65,535, and 64 under `Unlimited`.
  It was set per pattern consumer (`basic.qos` with `global = false`), so a server of n patterns
  took up to n times the bound and refused the excess. It is now one window for the channel the
  pattern consumers share (`global = true`, RabbitMQ's reading). The control consumer is on its
  own channel without acknowledgements, outside the window, so a held call's items and `cancel`
  still arrive. See S14.
- **Kafka:** the link counts each request record from its delivery until its `Ack` settles or is
  dropped (`InFlight`, held in the `Ack`'s closure). After each delivery `at_bound` checks the
  count: at the bound it pauses the assignment, then waits for the count to fall, the phase to
  leave `Serving`, or a record. The consumer is read while paused, which serves a rebalance; a
  record from a partition assigned since the pause is kept and the new assignment paused, and kept
  records are handed over first once there is room. On room it resumes the assignment if still
  serving; once draining it leaves the partitions paused and the lane ends. See S16.
- **The order that makes the link's count match the server's:** the dispatcher frees a call's
  permit before it settles the call's `Ack`, so a broker handing over the next request on the
  settlement finds the place free (`crates/ulo-rpc/src/dispatch.rs`, `Call::run`). `Link`'s doc
  states the order. See S15.

**Links that cannot hold work keep refusing**, unchanged in code: TCP, UDP, NATS, Redis and MQTT
refuse `unavailable` with `RetryAfter`, now at 1,024 unset. MQTT keeps its Receive Maximum at
65,535 and rumqttc's automatic acknowledgements.

**The WebSocket default widens** from 64 to 1,024 messages per connection. See S11.

### 8. The MQTT docs

- **Why MQTT refuses:** the crate doc gains a paragraph: over `Server::max_inflight`, 1,024 unset,
  a request is refused `unavailable` with `RetryAfter`, as on TCP, UDP, NATS and Redis; holding it
  needs the broker to pass an unacknowledged shared-subscription message to another member when
  the holder's session ends, which MQTT 5 §4.8.2 only recommends; Mosquitto 2.0.18 does not, in a
  probe of 60 runs over a clean DISCONNECT, an UNSUBSCRIBE then DISCONNECT, an aborted event loop,
  a killed process and a keep-alive timeout, at Receive Maximum 65,535 and 4; the behaviour is the
  broker's, EMQX or HiveMQ may pass the message on, and an opt-in verified per broker by the
  conformance drain scenario is not built (F365).
- **F355's full cause:** the drain paragraph adds that publishes queued for the instance behind a
  full window are dropped with its session, and that the 65,535 Receive Maximum keeps that window
  as wide as MQTT 5 allows. `RECEIVE_MAXIMUM`'s doc says the same and points at the crate doc.

### 9. S10: Kafka's blocking calls on its own threads, the backoff on the app's timer

- **`blocking`:** runs a call on a thread named `ulo-kafka-blocking` and answers its result
  through a oneshot, as `drop_detached` runs a consumer's drop. A thread that cannot be spawned
  runs the call in place, logged at `warn`; a call that panics answers an error. `Runtime` gains
  nothing.
- **Where:** the anchor's metadata reads, each committed-offsets attempt, and the watermark reads
  with the starting commit; the control consumer's metadata read. No `spawn_blocking` remains in
  the crate.
- **The backoff on the app's `Timer`, a clean restructure:** `anchor` runs three `blocking` steps
  and awaits between them, so `coordinator_retried` became async: each attempt a future, the wait
  `timer.sleep(backoff)`, the clock `timer.now()`. The `Timer` comes from `AppHandle::timer()` in
  `prepare`; `listen` refuses without one, which the RPC server's runtime check makes unreachable.
  The anchor's consumer is held in a `Detached`, so its last drop, on whichever thread, runs on a
  thread of its own.
- **The tests** drive `coordinator_retried` with ready futures and an injected clock through
  `now_or_never`; their assertions are unchanged.

## The tests

- **`crates/ulo-rpc/src/client.rs`,** `a_call_is_not_registered_on_a_connection_whose_reply_lane_ended`:
  over a link whose first connection's reply lane is empty, the test takes the connection, yields
  until the lane has ended, and requires `register` to refuse it and leave `pending` empty, then
  `ClientInner::register` to register on a second connection.
- **`crates/ulo-rpc-tcp/tests/late_goaway.rs`,**
  `a_connection_registered_after_the_drain_began_receives_goaway`: a TLS server; one raw TLS
  connection holds a call in flight, keeping the server draining; a second TCP connection is
  opened and left 200 ms for the server to accept it, then the drain starts, the first connection
  reads its `goaway`, and only then does the second complete its handshake; its first frame must
  be `goaway`.
- **`crates/ulo-rpc-conformance/src/cases/admission.rs`,** `over_the_bound`, stamped on every link:
  a server at `max_inflight(Count::Max(2))`, six calls at once, three to each of two patterns, each
  holding its place one second. Never more than two run at once (the probe's peak). A link
  declaring `native_backpressure` serves all six; any other serves two and refuses four
  `unavailable`, each with a `RetryAfter` detail.
- **`crates/ulo-http/tests/max_inflight.rs`,** two tests over the keeper backend:
  `a_request_over_the_bound_is_503_with_retry_after`, at `Count::Max(2)` with two requests held,
  and `the_default_bound_is_1024_requests`, at `Count::Default` with 1,024 held; each third or
  1,025th request is 503 with `Retry-After: 1`, and every held one is answered 200 once released.
- **`crates/ulo-ws/tests/stream_end.rs`,** three tests on a standalone gateway: a stream an
  interceptor discards for one frame reports nothing, the interceptor's own stream reports
  `Completed`, a written stream `Completed`. Each callback owns its channel's only sender, as in
  `ulo-rpc`'s `stream_end.rs`.
- **`crates/ulo-codegen-tests/tests/stream_end.rs`,** six tests on a new `Ends` service in
  `proto/probe.proto`, over a plain HTTP/2 client: discarded, replaced, written, an item's error
  (`CutOff(None)`, the client reading UNAVAILABLE after one tick), the post-deadline stream
  (`CutOff(Deadline)`, the client reading DEADLINE_EXCEEDED), and a reply the caller drops after
  its first tick (`CutOff(ClientCancelled)`).
- **`crates/ulo-rpc/tests/stream_end.rs`,** `a_stream_the_link_cannot_carry_reports_nothing`: the
  scripted link declares `UNARY_ONLY` and mounts a unary handler whose interceptor answers a
  stream; the reply is `err internal` and nothing is reported. The root module mounts that
  controller alone on the unary link, whose server refuses a streamed handler at startup.
- **`crates/ulo-rpc-udp/tests/close_read.rs`** also requires the close-time refusal's message to be
  "the server is shutting down".
- **`crates/ulo-rpc-kafka/src/link.rs`,** the three retry tests, rewritten for the async retry.

### Before and after

Each undo rewrote one span through `batch19/undo.py`, which ran the target, wrote the file back
byte for byte and compared hashes; every restore reported `ok`. The specs are in
`batch19/specs/`, the full output of each run in `batch19/runs/`.

| Code | Result |
| --- | --- |
| F364 unit test, `register`'s check rewritten to `if false && ..` | failed: "a call was registered on a connection whose reply lane had ended" |
| F364 unit test on the fix | passed |
| `drain_window` with a 30 ms sleep before the registration, on `20e98a30`'s `open` | 15 of 15 failed: "call 0 of 1 sent during the drain was not refused `Unavailable`, got: Err(Timeout)" |
| the same sleep on the fix, two rounds | 15 of 15 passed each |
| `late_goaway`, the phase check rewritten to `if false && ..` | failed: "the late connection: no frame within 3s" |
| `late_goaway` on the fix, three runs | passed each, 0.21–0.24 s |
| `over_the_bound` on TCP, the refusal's `RetryAfter` removed | failed: "a call over the bound was not refused `unavailable` with `RetryAfter`, got: .. Details([])" |
| `over_the_bound` on RabbitMQ, `basic.qos` back to `global: false` | failed: 4 served, 2 refused "the server is over its in-flight limit" |
| `over_the_bound` on Kafka, `at_bound` rewritten never to pause | failed: 2 served, 4 refused |
| `over_the_bound` on RabbitMQ, the `Ack` settled before the permit is freed, three runs | passed each (S15) |
| `over_the_bound` on the fix: TCP, RabbitMQ, Kafka | passed |
| HTTP, `Count::Default` mapped back to no bound | the default test failed: "a request over the bound was answered: fast"; the other passed |
| HTTP, the admission bound removed | both failed the same way |
| HTTP on the fix | 2 passed |
| WebSocket, the stream wrapped where the answer is built | discarded failed (`Some(CutOff(None))` against `None`); replaced failed (`Some(CutOff(None))` against `Some(Completed)`); written passed |
| WebSocket on the fix, three runs | 3 passed each |
| gRPC, `Tracked` put back inside `encode_stream` | discarded and replaced failed, each `Some(CutOff(None))` |
| gRPC, the trailers' status ignored (`report(true)`) | the item-error test failed: `Some(Completed)` against `Some(CutOff(None))` |
| gRPC, the post-deadline report removed | that test failed: `None` against `Some(CutOff(Some(Deadline)))` |
| gRPC, `CallBody`'s drop report removed | the abandonment test failed: `None` against `Some(CutOff(Some(ClientCancelled)))` |
| gRPC on the fix, three runs | 6 passed each |
| RPC, the stream wrapped before the capability check and dropped | the uncarried test failed: `Some(CutOff(None))` against `None` |
| RPC on the fix, three runs | 5 passed each |
| UDP `close_read`, the old "the server has closed" | failed, naming the message it got |
| UDP `close_read` on the fix, three runs | passed each |

Kafka's three unit tests are rewritten with the async retry and passed; no undo, the retry logic
being unchanged.

## Left for the transports DESIGN fold

- §2.6 (line 313): "HTTP and RPC wrap the stream where they write it. WebSocket and gRPC wrap it
  where the reply is built ... (F358, F359); the core's doc on `on_stream_end` states the rule and
  names both" no longer holds: every transport tracks the reply where it writes it, and the core's
  doc states the rule with no exception. gRPC's report is the dispatcher's, at the trailers.
- §2.8: `Count::Default` on `max_inflight` is `Count::DEFAULT_MAX_INFLIGHT`, 1,024, on every
  transport; over it a link declaring `native_backpressure` stops taking requests.
- §3's HTTP server settings: `max_inflight`'s `Default` is 1,024.
- §4.1's `Reply` paragraph (line 754): `fw_ws::Reply::Many(BoxStream<..>)`, the dispatcher
  tracking it where it writes it; the gateway's `max_inflight` default is 1,024 per connection.
- §5.2's flow-control paragraph: AMQP's prefetch is one window for the channel (`global = true`),
  the bound at `Max(n)` and at `Default`, 64 only under `Unlimited`; Kafka pauses its assignment at
  the bound until a request settles; the conformance list gains `over_the_bound`.
- §5.3's `Link::max_inflight` doc in the trait block, and the server's: the server frees a call's
  place before it settles the `Ack`; Kafka implements `max_inflight`.
- §5.3's link table: AMQP's row (prefetch per channel), Kafka's (pause at the bound, blocking calls
  on its own threads, the backoff on the app's timer), MQTT's (refuses over the bound; why, with
  the probe; messages behind a full window dropped with the session).
- §6.2: gRPC's refusal of a draining or closing call keeps its text; a reply stream's end is
  reported at its trailers.
- §5.4: a call registered on a connection whose reply lane ended in between goes over a new one.
- Decision 37 (line 1335): the anchor's broker calls run on threads of its own and its backoff on
  the app's `Timer`.

## Needs sign-off

Numbered on from batch 18's S1–S10, which this batch builds.

### S11. WebSocket's default widens from 64 to 1,024, per connection

Decision 7. WebSocket's `max_inflight` is per connection: the messages one connection may have in
flight before its read loop stops reading (`crates/ulo-ws/src/connection.rs`, `reading`), the
backpressure a TCP connection gives its client. Its built-in default was 64; the brief's "every
transport, wherever a `max_inflight` exists" made it 1,024 per connection, so a server of c
connections may run up to 1,024 × c handlers. There is no server-wide WebSocket bound. The
alternatives: keep 64 for WebSocket's per-connection bound, or read 1,024 as a server-wide bound
and add one.

### S12. A `goaway` still sets `open` outside the lock

Decision 1. `Conn::register` checks `open` under `pending`'s lock, and only the lane's end takes
that lock to set it. A call registering as a `goaway` arrives can still go over the connection; the
server is draining and refuses it `unavailable`. The alternative is to take the lock for `goaway`
too, at the cost of a lock on that frame.

### S13. WebSocket's close reason at the drain keeps "server shutting down"

Decision 6. The close frames WebSocket writes at the drain, 1001 with "server shutting down" for a
refused connect and for an idle connection the drain closes, end a connection rather than refuse a
call, and two tests pin the text. The alternative is to use "the server is shutting down" there
too.

### S14. RabbitMQ's prefetch is one window for the channel

Decision 7. `global = true` is RabbitMQ's reading of the flag, one limit shared by every consumer
on the channel; AMQP 0-9-1 reads it as connection-wide, and other brokers may. The pattern
consumers share one channel, so the window now bounds them together at the server's bound. The
alternative, per-consumer windows summing to the bound, divides it across patterns that may not
share load evenly.

### S15. The permit is freed before the `Ack` is settled

Decision 7. By reading, a holding link that frees its window on the settlement could hand over the
next request before the call's permit drops, and the server would refuse it. Neither broker showed
it: with the order reversed, RabbitMQ's `over_the_bound` passed three runs, its settlement going
through a spawned task and a network round trip. The order is kept as the cheaper guarantee; no
test pins it.

### S16. Kafka keeps a record that arrives while paused

Decision 7. A rebalance during a pause can assign a partition the pause did not cover; the record
read from it is kept in the lane and handed over once there is room, rather than refused. Several
rebalances during one pause can each add one record. The alternative is to hand it over at once,
over the bound, and let the server refuse it.

## Verification

Full, unfiltered output of every run is in the session scratchpad: `batch19/verify/` for the pass
below, with `summary-rpc.txt` and `summary-rest.txt` carrying each run's exit code and container
counts; `batch19/runs/` for the before/after runs and probes; `batch19/f364/` for the hunt.
`target/` was removed by a `cargo clean` the coordinator ran at 07:30, during the hunt's second
phase; every result below is from the rebuilt tree, and the two hunt failures are from binaries
copied out of `target/` before the clean.

Conformance, three consecutive runs per suite on the final tree, one suite at a time, test time as
libtest reports it; the brokers with `--features integration --test conformance --locked`:

| Suite | Run 1 | Run 2 | Run 3 | Result |
| --- | --- | --- | --- | --- |
| `ulo-rpc-tcp` conformance | 1.32 s | 1.31 s | 1.31 s | 26 passed |
| `ulo-rpc-tcp` conformance_cbor | 1.31 s | 1.31 s | 1.31 s | 26 passed |
| `ulo-rpc-tcp` conformance_tls | 1.33 s | 1.31 s | 1.31 s | 26 passed |
| `ulo-rpc-udp` conformance | 1.31 s | 1.31 s | 1.31 s | 25 passed, 1 ignored |
| `ulo-rpc-nats` | 8.86 s | 9.83 s | 11.45 s | 26 passed |
| `ulo-rpc-redis` | 12.20 s | 10.03 s | 9.91 s | 27 passed: the suite's 26 and the link test |
| `ulo-rpc-mqtt` | 12.26 s | 11.45 s | 10.98 s | 31 passed: the suite's 26 and five link tests |
| `ulo-rpc-kafka` | 40.51 s | 39.42 s | 36.57 s | 26 passed |
| `ulo-rpc-rabbitmq`, `ULO_CONFORMANCE_PARALLEL=6` | 134.67 s | 112.08 s | 154.64 s | 26 passed |

Each suite's 26th scenario is `over_the_bound`. RabbitMQ took five to seven times batch 18's
22–24 s: in the first run four scenarios printed "has been running for over 60 seconds" before
passing, their containers starting slowly; whether the host or the scenario's six one-second
calls account for the rest was not probed.

- HTTP, `cargo test -p <crate> --locked`: `ulo-http-axum` 38 passed, 1 ignored; `ulo-http-actix` 34
  passed, 5 ignored, `conformance_http2` among its binaries; `ulo-http-hyper` 44 passed, 3 ignored;
  `ulo-http-poem` 38, 1 ignored; `ulo-http-rocket` 38, 1 ignored; `ulo-http-salvo` 39, 2 ignored;
  `ulo-http` 20 passed, 9 ignored, `max_inflight`'s two included. `ulo-ws` 59 passed, 3 ignored;
  `ulo-grpc` 0 passed, 5 doc tests ignored, its tests living in `ulo-codegen-tests`;
  `ulo-graphql-ws` has no tests of its own, 0 run; `ulo-codegen-tests` 53 passed.
- `cargo check --workspace --all-targets --all-features` with
  `CFLAGS=-I/opt/homebrew/opt/openssl/include LDFLAGS=-L/opt/homebrew/opt/openssl/lib`, the same
  with default features, and `cargo +1.88 check --workspace --all-targets --exclude ulo-http-salvo
  --exclude ulo-graphql-async-graphql`: each exits 0 and prints the 17 known warnings in
  `crates/ulo/src` and no other.
- `cargo test --workspace --no-fail-fast` with the OpenSSL flags: 519 passed, 0 failed, 66 ignored
  across 135 test binaries: the 501 before, the client unit test, `late_goaway`, `over_the_bound`
  in the three TCP suites and UDP's, `max_inflight`'s two, WebSocket's three, gRPC's six and
  `ulo-rpc`'s `a_stream_the_link_cannot_carry_reports_nothing`.
- The tree checks: `cargo tree -p <crate> -e normal --all-features -i tokio` prints no tokio for
  `ulo`, `ulo-transport`, `ulo-net` and `ulo-rpc`; for `ulo-http` it prints tokio through
  `tokio-util`, the opt-in `tokio-io` feature stage b added, and without `--all-features` it prints
  nothing. The same command on `ulo-rpc-kafka` prints tokio, which shows the check reports a
  dependency it finds (`verify/tree.txt`). This batch changes no manifest but Kafka's.
- `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --lib -p <crate>` for `ulo`, `ulo-transport`,
  `ulo-http`, `ulo-ws`, `ulo-grpc`, `ulo-rpc`, `ulo-rpc-conformance`, `ulo-rpc-tcp`, `ulo-rpc-udp`,
  `ulo-rpc-redis`, `ulo-rpc-mqtt`, `ulo-rpc-rabbitmq` and `ulo-rpc-kafka`: each exits 0.
- `cargo +1.98.1 clippy -p <crate> --all-targets --no-deps --locked` for those thirteen and
  `ulo-codegen-tests`, and for `ulo-rpc-redis`, `ulo-rpc-mqtt`, `ulo-rpc-rabbitmq`, `ulo-rpc-kafka`
  and `ulo-rpc-nats` again with `--features integration`: each exits 0. Of the 47 warning locations
  outside `crates/ulo/src`, one falls on a line this batch wrote: `result_large_err` on the
  conformance crate's `bound`, whose signature gained the `max_inflight` parameter
  (`crates/ulo-rpc-conformance/src/cases/app.rs:499`); the lint is on its return type,
  `Result<App<Serving>, StartupError>`, which the batch did not change. The rest predate the batch, checked against `git diff -U0`'s hunks and
  the new files (`verify/clippy-locations.txt`). No macro changed, so `ulo-macro-lints` was not run.

### Containers

Every count before and after each run was 4: the user's seaweedfs, mailpit, postgres:18 and
redis:7, untouched. Every container a test started was removed by that test. The Docker engine
answered throughout.
