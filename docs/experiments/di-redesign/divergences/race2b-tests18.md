# Divergences: race 2b, tests batch 18, `confirms_drain` (S4), UDP's read at close (S6), RPC tracks the reply it writes (S5), Redis refuses what its ended stream no longer takes (F354), MQTT announces its Receive Maximum (F355), Kafka retries an unready coordinator (F357)

The thirty-third response, signed off 2026-10-08, settled three decisions and three bugs. The
capability is `confirms_drain`, `true` from `Capabilities::new`, with NATS and UDP declaring
`false`. UDP's `close` reads what its socket already holds and answers each request
`unavailable`. On RPC the dispatcher wraps a reply stream in `Tracked` where it writes it, so a
stream an interceptor discards reports nothing, and a stream the error handlers answer after a
passed deadline still reports `CutOff(Deadline)`. WebSocket and gRPC break the same rule and are
not changed: WebSocket's fix changes the public `ulo_ws::Reply::Many`, and gRPC's needs a
mechanism rather than a moved wrap (S1, S2; filed as F358 and F359). The rule is on
`Execution::on_stream_end` with that exception named (S3). Redis hands a frame over under the lock
its drain's watcher ends the stream under, and refuses a request its ended stream no longer takes.
Both sides of the MQTT link announce a Receive Maximum of 65,535; the RPC server's admission was
read first (decision 5). Kafka's `bind` retries the committed-offsets read on the three
coordinator errors within the call's existing timeout.

Files changed: `crates/ulo/src/execution/mod.rs` (doc only); `crates/ulo-http/src/{body.rs,
service.rs}` (doc only); `crates/ulo-rpc/src/{link.rs, transport.rs, dispatch.rs}`,
`crates/ulo-rpc/tests/stream_end.rs` (new); `crates/ulo-rpc-conformance/src/{lib.rs,
cases/drain.rs, cases/app.rs}`; `crates/ulo-rpc-nats/src/{lib.rs, link.rs}`;
`crates/ulo-rpc-udp/{Cargo.toml, src/lib.rs, src/link.rs}`, `crates/ulo-rpc-udp/tests/close_read.rs`
(new); `crates/ulo-rpc-redis/{Cargo.toml, src/link.rs, tests/conformance.rs}`;
`crates/ulo-rpc-mqtt/{Cargo.toml, src/lib.rs, src/link.rs, tests/conformance.rs}`;
`crates/ulo-rpc-kafka/src/link.rs`; `Cargo.lock`; F354, F355 and F357 appended and F358, F359 filed
in the workspace's `FRAMEWORK_GAPS.md`. The tree is `a98fa90b` plus this batch.

## The signatures

```rust
// ulo_rpc::Capabilities (renamed field and setter)
pub confirms_drain: bool,
pub const fn confirms_drain(self, confirms_drain: bool) -> Self;

// ulo_rpc::Reply (variant's field changed)
pub enum Reply {
    None,
    One(Data),
    Many(BoxStream<'static, Result<Data, BoxError>>), // was Tracked<BoxStream<..>>
}
```

`Capabilities::new` sets `confirms_drain` to `true`, the one capability it sets `true`;
`ulo_rpc_nats::Nats` and `ulo_rpc_udp::Udp` declare `confirms_drain(false)`. `Capabilities` has
no `Default` impl, so `new` is the one constructor changed (S4). A hand-built stream reply is
`Reply::Many(Box::pin(stream))`; `RpcCx::reply_stream` is unchanged. Every other public item is
unchanged. Private to `ulo-rpc-udp`: `State::socket`, `final_read`, and `refuse`'s `message`
parameter. Private to `ulo-rpc-redis`: `ServerSide::{end_if_idle, accept, refuse}` in place of
`deliver`, and `Calls::is_empty`. Private to `ulo-rpc-mqtt`: `RECEIVE_MAXIMUM`. Private to
`ulo-rpc-kafka`: `ANCHOR_TIMEOUT`, `RETRY_BACKOFF`, `RETRY_BACKOFF_MAX`, `coordinator_retried`
and `coordinator_not_ready`. `socket2` becomes a dependency of `ulo-rpc-udp` and `ulo-transport`
of `ulo-rpc-redis`, both already in the workspace; `ulo-rpc-mqtt`'s tokio dev-dependency gains
`net` and `io-util`.

## Decisions

### 1. S4: `confirms_drain`, `true` unless declared

- **The field and its doc** (`crates/ulo-rpc/src/link.rs`): the drain returns only once nothing
  more is on its way to the server; `true` unless a link declares otherwise, so a link that does
  not declare it is held to the confirmed drain; a link declaring `false` names what a caller then
  sees. Its UDP clause names only a datagram arriving after the socket closes (decision 2).
- **`new`'s doc** says `confirms_drain` is the one capability it sets `true`.
- **The suite:** `times_out_during_drain` reads `!capabilities.confirms_drain`
  (`crates/ulo-rpc-conformance/src/cases/drain.rs`); the scenario's doc and the crate's "No
  scenario passes on silence" paragraph name `confirms_drain(false)` and say a link that does not
  declare it is held to the confirmed drain. `conformance_suite!` stamps `drain_window` for every
  link as before: the capability changes the assertion, not the stamping.
- NATS's and UDP's crate docs and NATS's request-lane comment name `confirms_drain` as `false`.

### 2. S6: UDP reads its socket at `close`

- **Where:** `Udp::close` aborts the receive task and awaits it, then takes the server's socket
  from `State::socket`, set by `listen`, and runs `final_read` before dropping it
  (`crates/ulo-rpc-udp/src/link.rs`). Aborting first leaves no other reader.
- **The read:** without waiting, until the receive buffer is empty: each datagram decoded, each
  `req` or `open` answered `err` of kind `unavailable`, "the server has closed", under the sender's
  own id through the `refuse` the receive loop uses after its stream ends, which now takes the
  message; anything else is dropped. A sender's ICMP refusal surfacing as `ConnectionReset` or
  `ConnectionRefused` is skipped, as in the receive loop; `WouldBlock` ends the read; any other
  error ends it at `debug`. A count of the requests answered is logged at `debug`.
- **Through a duplicate, not tokio:** the read goes through `socket2::SockRef::try_clone`, set
  non-blocking, as a `std::net::UdpSocket`. tokio's `try_recv_from` tries the read only while its
  reactor holds the socket readable (`tokio-1.53.2/src/runtime/io/registration.rs:188-193`), and a
  datagram that arrived since the reactor's last turn would be left unread. The test below did not
  reach that case: with `try_recv_from` in place of the duplicate it passed, the current-thread
  runtime turning its reactor while `close` awaits the aborted task. The claim rests on tokio's
  source; see S6.
- **What stays unreachable:** a datagram arriving after the socket closes. The declaration's
  comment says so, and that ICMP port-unreachable can reach a connected client socket but is often
  filtered and is not relied on. UDP keeps `confirms_drain(false)`.

### 3. S5: RPC wraps the reply stream where the dispatcher writes it

- **The move:** `RpcCx::encoded_stream`, which `reply_stream` and a handler's stream answer go
  through, returns the bare stream (`crates/ulo-rpc/src/transport.rs`). `answer` wraps
  `Outcome::Done(Ok(Reply::Many(stream)))` in `Tracked` before `stream_reply` writes it
  (`crates/ulo-rpc/src/dispatch.rs`). A stream an interceptor drops before answering something
  else, or an event handler's stream replaced by `Reply::None`, reports nothing.
- **HTTP's one exception, kept:** `Outcome::Expired(Some(Ok(Reply::Many(stream))))`, a stream the
  error handlers answered after a passed deadline, is wrapped and dropped unwritten, with a comment
  saying why: it was the reply, so it reports `CutOff(Deadline)`. HTTP's post-deadline path passes
  the error handlers' body through `tracked` before reading it for the same reason
  (`crates/ulo-http/src/service.rs`, `completed`). The module doc of `dispatch.rs` says the dropped
  stream reports `CutOff(Deadline)`.
- **A stream the link cannot carry:** `stream_reply` answers `internal` when the link carries no
  streamed shape and drops the stream it was handed, now already wrapped, so it reports
  `CutOff(None)` as before. It was the reply the dispatcher went to write. See S5.
- **`Reply`'s doc** says the dispatcher tracks a stream once it writes it as the call's reply and
  that a stream an interceptor discards reports nothing. The conformance app's hand-built
  `Reply::Many` drops its `Tracked::new`.

### 4. S5: WebSocket and gRPC against the same rule

Both break it. Neither is changed; a temporary probe test showed each, and was removed.

- **WebSocket:** `Answer::<ViaStream>::answer` wraps the handler's stream when the answer is built
  (`crates/ulo-ws/src/__private.rs:291`) and `ulo_ws::Reply::Many` carries the `Tracked`
  (`crates/ulo-ws/src/transport.rs:191`). A probe gateway whose interceptor dropped the handler's
  two-item stream and answered one frame got `{"id":"d","data":0}` on the wire and recorded
  `[CutOff(None)]` (`runs/ws-discard-probe.txt`). The fix is RPC's: `Reply::Many` holding the bare
  stream and `message` wrapping it where it pumps it (`crates/ulo-ws/src/connection.rs:1072`).
  `Reply::Many`'s field is public and its type changes, which is the brief's example of a design
  change. See S1; filed as F358.
- **gRPC:** `encode_stream` wraps the item stream in `Tracked` inside the encoded body
  (`crates/ulo-grpc/src/dispatch.rs:352-366`); `Reply` is an erased
  `http::Response<tonic::body::Body>`. A probe service whose interceptor dropped the handler's
  three-item stream and answered a one-item stream delivered `[1]` to the client and recorded
  `[CutOff(None)]`, not `Completed` (`runs/grpc-discard-probe.txt`). No public type changes, but the
  wrap cannot move: the dispatcher sees an encoded body (`CallBody::new`, `dispatch.rs:174`), so
  tracking where it writes needs a mechanism of its own. See S2; filed as F359.
- **The others:** HTTP follows the rule since batch 14. `ulo-graphql-ws` wraps the subscription's
  stream as it spawns the task that writes it (`crates/ulo-graphql-ws/src/gateway.rs:331`), with
  nothing between that could discard it.

### 5. S5: the sentence on `Execution::on_stream_end`

- **The core:** `Execution::on_stream_end` (`crates/ulo/src/execution/mod.rs`; the method is in
  `ulo`, not `ulo-transport`, which holds `Tracked`) carries batch 14's sentence, "it reports the
  reply actually written; a stream discarded before the response is sent is not a reply and
  reports nothing", and a second naming WebSocket and gRPC, where a reply stream is tracked where
  it is built and one an interceptor discards reports `CutOff`. See S3.
- **`ulo-http`:** the sentence is removed from `HttpBody::stream` and from step 8 of `AppService`'s
  doc; each keeps what is HTTP's own, the service wrapping the body as it answers.

### 6. F354: Redis

- **The lock:** `ServerSide::accept` holds `deliveries`' lock while it holds a call and hands its
  frame over; the drain's watcher ends the stream through `end_if_idle`, which takes the same lock
  and takes the sender only when `Calls` holds nothing, looping on the count otherwise
  (`crates/ulo-rpc-redis/src/link.rs`). Both take `deliveries` before `Calls`' own lock. `close`
  takes the sender before it clears the table, as MQTT's does.
- **The refusal:** a `req` or `open` the stream no longer takes is not held; `accept` returns its
  wire id and reply channel, and `on_message` publishes `err` of kind `unavailable`, "the server
  is draining", under the wire id, which the client maps back to its own. A delivery the sender
  refuses is released from `Calls` and refused the same way. Control frames and events are dropped
  once the stream has ended, as before.
- **Inline, not spawned:** `on_message` awaits the refusal's publish on the server lane, which
  awaits nothing else per message; MQTT's refusal is queued without waiting because it runs on the
  event loop. See S8.
- **The probe caught the first version:** its `let sender = deliveries.as_ref()?;` returned with no
  refusal once the stream had ended; with the lane probe it failed exactly as the old link did. The
  `else` branch now returns the refusal.
- **RabbitMQ is unchanged:** a delivery it drops after its stream ended leaves its `Ack` unsettled,
  and AMQP requeues it at channel close, which `holds_unserved` declares.

### 7. F355: MQTT's Receive Maximum, after the admission check

**The admission check.** `Admission` is a semaphore that refuses at once and never queues
(`crates/ulo-transport/src/admission.rs`, `try_admit`). The RPC dispatcher calls it for each call
the serve loop routes and, over the limit, refuses a request `unavailable` with `RetryAfter`
through `refuse` and leaves an event unacknowledged (`crates/ulo-rpc/src/dispatch.rs:186-195`).
The bound is `Server::max_inflight`: at `Count::Max(n)` over-limit requests are refused; at
`Count::Default`, the default, and `Unlimited` there is no bound and every request is admitted
(`crates/ulo-rpc/src/server.rs:157-160`, documented "unbounded at `Count::Default`"). Between the
MQTT event loop and the serve loop deliveries cross an unbounded channel, which the serve loop
drains routing each delivery before it awaits anything. rumqttc acknowledges a QoS 1 publish as its
event loop reads it (`rumqttc-0.25.1/src/v5/state.rs:330-335`), before the server sees it, so the
broker's window is replenished by the read whatever the server does, and the Receive Maximum was
never the server's backpressure. Nothing that exceeds the server's bound is buffered without
limit: refused under a bound, admitted at the default. See S9.

**What rumqttc allows.** `MqttOptions::set_receive_maximum(Option<u16>)` writes the CONNECT
property (`rumqttc-0.25.1/src/v5/mod.rs:350`); `set_outgoing_inflight_upper_limit` caps the
broker's window for this client's own publishes and is not this. The link builds both sides' options
in one `options`, so both announce 65,535 (`RECEIVE_MAXIMUM`). See S7.

**What it changes, probed.** MQTT 5 §3.1.2.11.3 reads an absent Receive Maximum as 65,535, so the
announcement could have been a no-op. A temporary probe against Mosquitto 2.0.18, a subscriber
with manual acknowledgments that acknowledged nothing and a publisher sending 200 QoS 1 messages:
without the property 20 were delivered, with 65,535 all 200 (`runs/f355-mosquitto-inflight-probe.txt`).
Mosquitto applies its `max_inflight_messages`, 20 unset, when the client announces nothing. The
probe was removed; the test below keeps the effect pinned through the link itself.

**Documented:** the crate doc's drain paragraph states the remaining condition, a request lost at
shutdown only when more requests are outstanding to the instance than the broker's flow control
allows, the announced 65,535 or a smaller cap the broker applies itself; the drain's comment and
`RECEIVE_MAXIMUM`'s doc say the same. `confirms_drain` stays `true`, and every scenario keeps its
strict assertion.

### 8. F357: Kafka retries an unready coordinator

- **Where:** `anchor`'s `committed_offsets` call alone, through `coordinator_retried`
  (`crates/ulo-rpc-kafka/src/link.rs`). `fetch_metadata` and `fetch_watermarks` do not ask the group
  coordinator and keep failing on their first error; the commit's failure was already tolerated.
- **Which errors:** `KafkaError::MetadataFetch` carrying `NotCoordinator`,
  `CoordinatorLoadInProgress` or `CoordinatorNotAvailable`, the variant rdkafka 0.39's
  `committed_offsets` returns (`rdkafka-0.39.0/src/consumer/base_consumer.rs:650-651`). Any other
  error fails `bind` at once, as U19 requires.
- **The timeout:** the call's existing 10 s, now `ANCHOR_TIMEOUT`, is one deadline for the read and
  its retries, each attempt given what remains; a retry is made only while its wait still fits
  before the deadline, and the last error is returned otherwise. No knob.
- **The backoff and its clock:** 100 ms, doubling to 1 s, slept with `std::thread::sleep`. The app's
  `Timer` is not reachable from a link: `Link::prepare` receives an `AppHandle`, which exposes no
  timer, and `anchor` already runs inside `spawn_blocking`, where an async timer could not be
  awaited anyway. `coordinator_retried` takes `sleep` and `now` as parameters, which is what lets
  the tests drive it. See S10.

## The tests

- **`crates/ulo-rpc/tests/stream_end.rs`, four tests,** over a scripted link the test feeds,
  current-thread runtimes. Each handler registers a callback that owns the only sender of a channel
  the test reads, so the channel closes when the execution ends: `None` is read for a stream that
  reported nothing, with no wait for a report that might still come. The test's own `stop` ends the
  inbound stream before closing, so the drain waits for nothing.
  - `a_stream_an_interceptor_discards_reports_nothing`: the interceptor drops the handler's stream
    and answers `0`; the reply is `res 0` and nothing is reported.
  - `the_stream_replacing_a_discarded_one_reports_its_own_end`: the interceptor answers a stream of
    its own; its two items and `end` are written and `Completed` is reported.
  - `a_written_stream_reports_completed`.
  - `a_stream_the_error_handlers_answer_after_the_deadline_reports_cut_off`: `deadline-ms: 20`, a
    handler that never answers, an error handler answering a stream; the reply is `err timeout` and
    `CutOff(Some(Deadline))` is reported.
- **`crates/ulo-rpc-udp/tests/close_read.rs`,**
  `close_answers_the_requests_its_socket_already_holds`: on a current-thread runtime the link
  listens, the receive task first waits on the empty socket, five requests are sent with blocking
  sends and no await, and `close` runs; the caller must read five `err` frames of kind
  `unavailable`, one per id. No widened window is needed: the test's task holds the receive task off
  the socket until `close` has aborted it.
- **`crates/ulo-rpc-redis/tests/conformance.rs`,**
  `a_request_read_after_the_drain_ended_the_inbound_stream_is_refused`, against its own Redis
  container: a request published, then `drain`; if the inbound stream hands the request over, the
  test answers it and the caller must receive the `res`; if the stream ends first, the caller must
  receive `err` of kind `unavailable`.
- **`crates/ulo-rpc-mqtt/tests/conformance.rs`,** `the_broker_holds_back_no_request_for_flow_control`,
  against its own Mosquitto container: the server link reaches the broker through a relay that,
  once muted after the server subscribed, forwards nothing the server writes, so no PUBACK reaches
  the broker; 50 requests are published and all 50 must reach the server.
- **`crates/ulo-rpc-kafka/src/link.rs`, three unit tests** over an injected clock that `sleep`
  advances: each of the three codes retried until the read succeeds, with waits of 100 ms then
  200 ms; another error returned after one attempt with no wait; an always-failing coordinator
  returning its last error with less than the timeout slept, the first attempt given the whole
  timeout and each later one less. No broker test: a test cannot hold a broker's coordinator unready
  on demand.

### Before and after

Each undo rewrote one line through a script that ran the target and wrote the file back byte for
byte, checked by hash; the full output of every run is in the scratchpad (`batch18/runs/`).

| Code | Probe | Result |
| --- | --- | --- |
| `stream_end.rs` against `a98fa90b`'s `transport.rs` and `dispatch.rs` | none | 2 failed: the discarded stream "left: Some(CutOff(None)) right: None"; the replacing stream "left: Some(CutOff(None)) right: Some(Completed)"; the written and post-deadline tests passed |
| fix, three runs | none | 4 passed each, 0.02–0.03 s |
| fix, the post-deadline wrap rewritten to `drop(stream)` | none | the post-deadline test failed, "left: None right: Some(CutOff(Some(Deadline)))"; the other three passed |
| `close_read.rs`, the final read rewritten to `let _ = (&socket, self.codec);` | none | failed after the caller's 5 s read timeout: "0 of 5 requests left in the socket at close were answered" |
| fix, three runs | none | passed each, 0.00 s |
| fix, the duplicate's `recv_from` rewritten to tokio's `try_recv_from` | none | passed (decision 2) |
| Redis test against `a98fa90b`'s `link.rs` | server lane 50 ms behind each message | failed: "a request read after the inbound stream ended was not refused `unavailable`, got: None" |
| first version of the fix | 50 ms behind | failed the same way (decision 6) |
| fix | 50 ms behind | passed |
| fix, three runs | none | passed each, 0.46–0.71 s |
| MQTT test, `set_receive_maximum` rewritten out | none | failed: "20 of 50 requests reached the server unacknowledged; then: None" |
| fix, three runs | none | passed each, 0.70–1.46 s |
| Kafka unit tests on the fix, then again in the crate and workspace passes | none | 3 passed each |
| the retry condition rewritten to `false && ..` | none | the retry test failed: "NotCoordinator was not retried until the read succeeded"; the other two passed |
| the retry condition rewritten to accept any error | none | the other-error test failed: "an error other than the coordinator's three was retried", 13 attempts |

The 50 ms probe is a `tokio::time::sleep` before each `on_message` in the Redis server lane,
inserted by the script and removed with the restore. The Kafka tests are new with the fix, so the
undo of its decision line stands for the run against the code before it.

Runs set apart and not counted, kept in `runs/invalid/` and `runs/superseded/`: a first UDP run
that compiled `ulo-rpc` mid-edit and failed to build; the UDP runs before the test's receive task
was made to wait on the empty socket first; a first RPC run whose `stop` left the inbound stream
open, each test then waiting out the drain (10 s); a probe that failed to compile; and the Redis
fix runs of the first version.

## Left for the transports DESIGN fold

- §2.6 (line 313): "The rule is the HTTP service's, not `on_stream_end`'s own: on RPC a
  `Reply::Many` the error handlers answer after a passed deadline is dropped unwritten, and its
  `Tracked` reports `CutOff(Deadline)` from the drop (§5.2)" no longer holds. The rule is
  `on_stream_end`'s on HTTP and RPC, the post-deadline stream reporting `CutOff(Deadline)` on both
  as the reply dropped unwritten; WebSocket and gRPC report a discarded stream `CutOff` (F358,
  F359).
- §4.1's `Reply` paragraph (line 754): `fw_ws::Reply::Many(Tracked<..>)` stands, and with it the
  WebSocket exception, until S1 is settled.
- §5.1's `Reply` paragraph (line 837): `fw_rpc::Reply` is `None | One(Data) | Many(BoxStream<..>)`,
  the dispatcher wrapping the stream in `Tracked` where it writes it.
- §5.2's conformance paragraph (line 878): `unconfirmed_drain` is `confirms_drain`, a declared
  `false` where the caller's `Timeout` passes, and a link declaring nothing held to the confirmed
  drain.
- §5.3's `Link::drain` doc in the trait block (line 897): "declares `unconfirmed_drain`" is
  "declares `confirms_drain(false)`".
- §5.3's `Capabilities` block (line 934): the field is `confirms_drain`, its comment the positive
  statement; the paragraph after it (line 944), "`new` sets `holds_unserved`, `durable_replies` and
  `unconfirmed_drain` to `false`", now has `new` set `confirms_drain` to `true`, and "under
  `unconfirmed_drain` a call..." reads under `confirms_drain(false)`.
- §5.3's link table, UDP's row (line 955): `close` answers `unavailable` the requests its socket
  already holds; a datagram arriving after the socket closes is the one lost, declared
  `confirms_drain(false)`, ICMP not relied on. MQTT's row (line 959): both sides announce a Receive
  Maximum of 65,535, and a request is lost at shutdown only when more are outstanding than the
  broker's flow control allows.
- §5.3's shutdown paragraph (line 964): "declares `unconfirmed_drain`" (twice) is
  `confirms_drain(false)`; Redis's part gains the watcher ending the stream under the lock the
  lane hands over under and the lane refusing what the ended stream no longer takes; MQTT's part
  gains the announced Receive Maximum and the remaining condition; UDP's gains the read at `close`.
- Decision 37 (line 1335): the anchor's committed-offsets read retries the three coordinator
  errors within its timeout.
- Decision 43 (line 1341): "NATS and UDP declare `Capabilities::unconfirmed_drain`" and its
  naming rationale ("the name states what the link cannot do") are replaced by `confirms_drain`,
  `true` by default, stating what a link does as the other capabilities do; UDP's clause narrows to
  a datagram arriving after the socket closes.
- X5's row (line 1186) is consistent and needs no change.

## Needs sign-off

### S1. WebSocket is left reporting a discarded stream

Decision 4: the fix is RPC's, moving the wrap from `Answer::<ViaStream>::answer` into `message`, and
changes the public `ulo_ws::Reply::Many` from `Tracked<BoxStream<..>>` to `BoxStream<..>`. The
brief names a public type changing shape as the condition to stop and report. The alternative is to
apply it, as signed off for RPC's identical type. F358.

### S2. gRPC is left reporting a discarded stream

Decision 4: no public type changes, but the dispatcher sees an encoded body, so the wrap cannot
move. Two mechanisms: a private arming handle `encode_stream` leaves in the response's extensions
and the dispatcher arms where it writes, keeping the item-level outcome; or a frame-level `Tracked`
at the dispatcher for a body `encode_stream` marks, which moves `Completed` to after the trailers
and has to be ordered against `CallBody`'s own end and drop. F359.

### S3. The core's `on_stream_end` doc names the WebSocket and gRPC exception

Decision 5: the response moved the sentence to the core so it could stand without a per-transport
exception; with S1 and S2 open it cannot, and the doc states both. The alternative is to keep the
sentence out of the core, on `ulo-http` and `ulo-rpc` alone, until both are fixed.

### S4. `Capabilities` has no `Default`

Decision 1: the brief said `new` and `Default` set every capability `false`; only `new` exists, and
no `Default` was added. One would have to pick a `DeliveryMode`.

### S5. A stream the link cannot carry still reports `CutOff(None)`

Decision 3: on a link carrying no streamed shape, `stream_reply` answers `internal` and drops the
reply stream it was handed, wrapped. It is reported as the reply the dispatcher went to write, as
the post-deadline stream is. The alternative is to wrap only once the link's capability check
passes, so such a stream reports nothing.

### S6. UDP's read at close goes through a duplicate socket

Decision 2: `socket2` for a duplicate read from the kernel, on tokio's own source rather than an
observed failure; tokio's `try_recv_from` passed the same test. The alternative is tokio's
`try_recv_from`, with no new dependency, accepting that a datagram its reactor has not yet marked
readable is left unanswered. The refusal reads "the server has closed", against the drain's "the
server is draining".

### S7. Both MQTT sides announce the Receive Maximum

Decision 7: one `options` builds both, so the client's reply topic also loses Mosquitto's
20-publish window. The response named the link's CONNECT without a side.

### S8. Redis refuses on the server lane, awaiting the publish

Decision 6: a slow publish delays the lane's next message. The alternative is to spawn each
refusal, which the link would then have to finish before `close`, as the dispatcher's refusals are.

### S9. The admission check, read as no stop

Decision 7: over `Server::max_inflight` a request is refused `unavailable`; at `Count::Default`, the
default, every request is admitted, none refused and none queued. The Receive Maximum never bounded
what the server takes in, rumqttc acknowledging each publish as it reads it, so announcing 65,535
changes the window on the wire and not the server's intake. The build continued on that reading.

### S10. Kafka's backoff runs on the blocking thread

Decision 8: `std::thread::sleep` inside `spawn_blocking`, 100 ms doubling to 1 s, the 10 s
per-call timeout reused as one deadline for the read and its retries. The app's `Timer` is not
reachable from a link. The alternative, an `AppHandle` accessor for the timer, is a core change.

## Verification

Full, unfiltered output of every run is in the session scratchpad, `batch18/verify/` for the pass
below and `batch18/runs/` for the before/after runs and probes; `verify/summary.txt` carries each
run's exit code and container counts.

Conformance, three consecutive runs per suite on the final tree, one suite at a time, test time as
libtest reports it; the brokers with `--features integration --test conformance --locked`:

| Suite | Run 1 | Run 2 | Run 3 | Result |
| --- | --- | --- | --- | --- |
| `ulo-rpc-tcp` conformance | 1.31 s | 1.31 s | 1.31 s | 25 passed |
| `ulo-rpc-tcp` conformance_cbor | 1.31 s | 1.31 s | 1.31 s | 25 passed |
| `ulo-rpc-udp` conformance | 1.31 s | 1.31 s | 1.31 s | 24 passed, 1 ignored |
| `ulo-rpc-nats` | 6.30 s | 7.58 s | 12.43 s | 25 passed |
| `ulo-rpc-redis` | 7.90 s | 7.29 s | 7.47 s | 26 passed: the suite's 25 and the link test |
| `ulo-rpc-mqtt` | 8.70 s | 8.14 s | 10.12 s | 30 passed: the suite's 25 and five link tests |
| `ulo-rpc-kafka` | 30.07 s | 23.94 s | 29.90 s | 25 passed |
| `ulo-rpc-rabbitmq`, `ULO_CONFORMANCE_PARALLEL=6` | 22.23 s | 23.86 s | 22.86 s | 25 passed |

Kafka bound in every run; F357's failure did not occur, so the retry was not exercised by a broker.

- HTTP: `cargo test -p <crate> --locked` for `ulo-http-axum` (38 passed), `ulo-http-actix` (34
  passed, 4 ignored), `ulo-http-hyper` (36 passed, 2 ignored; `route_timeout` 6 passed),
  `ulo-http-poem` (38), `ulo-http-rocket` (38), `ulo-http-salvo` (39), and `ulo-http` (7 and 10
  passed); `ulo-ws` (every target passed, 3 doc tests ignored); `ulo-grpc` (5 doc tests ignored, its
  tests living in `ulo-codegen-tests`); `ulo-codegen-tests` (every target passed); `ulo-rpc`
  (`drain_window` 3, `stream_end` 4, unit 1); `ulo-rpc-udp` (`client_close`, `close_read`,
  `drain_end` and the suite); `ulo-rpc-kafka --lib` (3).
- `cargo check --workspace --all-targets --all-features` with
  `CFLAGS=-I/opt/homebrew/opt/openssl/include LDFLAGS=-L/opt/homebrew/opt/openssl/lib`, and `cargo
  +1.88 check --workspace --all-targets --exclude ulo-http-salvo --exclude
  ulo-graphql-async-graphql`: each prints the 17 known warnings in `crates/ulo/src` and no other.
- `cargo test --workspace --no-fail-fast` with the OpenSSL flags: 446 passed, 0 failed, 64 ignored
  across 121 test binaries: batch 17's 438, the four `stream_end.rs` tests in `ulo-rpc`,
  `close_read.rs` in `ulo-rpc-udp`, and the three Kafka unit tests. The Redis and MQTT link tests are
  behind `integration` and run in their suites above.
- `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --lib -p <crate>` for `ulo`, `ulo-http`,
  `ulo-rpc`, `ulo-rpc-conformance`, `ulo-rpc-nats`, `ulo-rpc-udp`, `ulo-rpc-redis`, `ulo-rpc-mqtt`
  and `ulo-rpc-kafka`: each passes; the only warnings printed are the core's 17.
- `cargo +1.98.1 clippy -p <crate> --all-targets --no-deps --locked` for the same nine, and for
  `ulo-rpc-redis`, `ulo-rpc-mqtt`, `ulo-rpc-kafka` and `ulo-rpc-nats` again with `--features
  integration`: each exits 0, and none of the 36 warning locations printed outside `crates/ulo/src`
  falls on a line this batch wrote or in its two new test files, checked against `git diff -U0`'s
  hunks (`verify/clippy-locations.txt`). Among them are batch 17's list, `crates/ulo-rpc/src/link.rs:136`
  and `:179`, `client.rs:71`, `extract.rs`, MQTT's `clone_on_copy` now at `link.rs:655`, the
  conformance crate's two, and `ulo-rpc-kafka`'s `let_underscore_future` in `Detached`'s drop, now at
  `link.rs:997`. Nothing generated changed, so `ulo-macro-lints` was not run.

### Containers

At the batch's start two containers ran, postgres:18 and redis:7; the user's seaweedfs and mailpit
containers had exited 22 minutes earlier and were not touched. Every count before and after each
run in this batch was 2 until both came back up, by no action of this session, during the
workspace test run, whose count reads 2 before and 4 after; `docker ps` then listed the user's four
and nothing else. Every container the tests started was removed by its test; the two
`docker run --rm` reads of Mosquitto's configuration and the F355 probe's two Mosquitto containers
included. The Docker engine answered throughout.
