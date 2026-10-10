# Divergences: race 2b, tests batch 22, one writer per link with a client stream shown in order and a `cancel` shown after its request (S4), a client's unusable link refused where the client is built (S1), `BroadcastAdapter::prepare` (S9), F369 and F370 fixed, and CI's broker job renamed (S10)

The forty-first response, signed off 2026-10-10, answers `runtime-e1.md`. UDP, Redis, RabbitMQ and
Kafka send every frame through one writer task per link side, or per socket on UDP, fed by a
bounded queue of 64 in the order sends are first polled, the shape TCP's writer already had; the
per-frame `Tokio::run` tasks are gone from their send paths and reply paths. NATS and MQTT already
funnel every frame through their library's one command channel. The brokers' streamed-request
gating moved into the writer, so no task per call publishes beside it. A conformance scenario sends
256 items back to back and requires them in order with `in_end` last; a link-level check, called by
the TCP and Redis suites, sends 200 request and `cancel` pairs and requires every `cancel` after its
request. Against Redis's link as at `HEAD`, 64 of the 200 `cancel`s overtook their requests; the
ordered client stream passed there, because each frame's task finished before the next frame was
sent (decision 2, S1). `Link::usable` is new: `RpcClient::new` returns `Result<RpcClient,
UnusableLink>` and `RpcClientModule`'s init hook fails `connect` with it. `BroadcastAdapter::prepare`
is defaulted, and both hubs call it. MQTT's event loop is aborted with a dropped `connect` or
`listen` (F369); Kafka's `close` commits on a thread of its own, and the inline commit was found to
block without bound against a frozen broker (F370). CI's job is `broker integration`.

Files changed: `Cargo.lock` (an edge from `ulo-rpc-redis` to `bytes`); `.github/workflows/ci.yml`;
`Makefile` (a comment); `crates/ulo-rpc/{src/link.rs, src/client.rs, src/client_module.rs,
src/lib.rs, tests/runtime.rs}`; `crates/ulo-rpc-conformance/src/{lib.rs, cases/mod.rs,
cases/app.rs, cases/streams.rs, cases/threads.rs, cases/order.rs (new)}`; for each of the seven
links `src/link.rs` and `tests/handle.rs`, with `crates/ulo-rpc-redis/Cargo.toml`,
`crates/ulo-rpc-{tcp,redis,kafka}/tests/conformance.rs` and `crates/ulo-rpc-mqtt/tests/abandoned_connect.rs`
(new); `crates/ulo-ws/{src/broadcast.rs, src/rooms.rs, src/handoff.rs, src/table.rs,
tests/handoff.rs}`; `crates/ulo-ws-redis/{src/lib.rs, tests/handle.rs}`; in the workspace's
`FRAMEWORK_GAPS.md`, notes under F369 and F370 and F376 filed. The tree is `269afe50` plus this
batch.

## The signatures

```rust
// ulo_rpc::Link, added, defaulted
fn usable(&self) -> Result<(), BoxError> { Ok(()) }   // the seven links: `self.runtime().map(drop)`

// ulo_rpc, changed
impl RpcClient {
    pub fn new<L: Link>(link: L, runtime: Arc<dyn Runtime>) -> Result<RpcClient, UnusableLink>; // was `-> RpcClient`
}

// ulo_rpc, new
#[derive(Debug)]
pub struct UnusableLink { /* private: the call site, `Link::NAME`, the link's refusal */ }
impl UnusableLink { pub fn link(&self) -> &'static str; }
impl Display for UnusableLink {}   // "`RpcClient::new` was given a tcp link it cannot use: …", or `RpcClientModule::for_root`
impl Error for UnusableLink {}     // `source()` is the link's refusal

// ulo_ws::BroadcastAdapter, added, defaulted
fn prepare(&self) -> Result<(), BoxError> { Ok(()) }  // `ulo_ws_redis::Redis`: `self.runtime().map(drop)`

// ulo_rpc_conformance, a scenario stamped as `client_stream_in_order`, and a check stamped by no macro
pub mod cases {
    pub mod streams { pub async fn client_stream_in_order<B: Broker>(); }
    pub mod order { pub async fn cancel_follows_its_request<B: Broker>(); }
}
```

Unchanged: `Outbound`, `ReplyPath`, `Inbound`, `Delivery`, `Ack`, `Capabilities`, `Server`,
`RpcClientModule`'s builder, `Tokio`, `Rooms`, `GatewayTable`'s signatures, the `with_handle`
builders. `Outbound`'s doc states the ordering rule.

Private: on UDP, `Datagram`, `write`, `write_datagrams` and the `Sending` trait, with `State::writes`
and `reply_path` taking the writer's queue; on Redis, `Outgoing`, `client_writer`, `Publish`,
`server_writer` and `write`, `ServerSide::writes` replacing its `publisher`; on RabbitMQ and Kafka,
`Job` (`Send` or `Opened`), `client_writer`, a server writer and `write`, `ClientCall::Streaming {
opened, held }` replacing the per-call gate and queue, and on Kafka `enqueue` and `refused` replacing
`deliver`; MQTT's `AbortOnDrop` holds an `Option` and has `new` and `disarm`. The `pump` functions of
RabbitMQ and Kafka, and the `runtime` field of their client sides, are gone. In `ulo-ws`,
`Hub::prepare`.

Dependencies: `ulo-rpc-redis` names `bytes`, already in its tree through `ulo-rpc`. Nothing else.

## Decisions

### 1. S4: the writer per link, and what each link does

| Link | Writer | Fed by |
| --- | --- | --- |
| TCP | one per connection, as before | the client's `send`, the reply path, `refuse`'s answer, the drain's and a late connection's `goaway` |
| UDP | one per socket: each client socket's, ended by `close`'s epoch or the client dropping its sends; the server socket's, ended once every sender is gone | the client's `send`; the reply path, `refuse` in the receive loop and `final_read` at `close` on the server |
| NATS | async-nats's connection task, through its command channel | unchanged: publishes stay inline, a streamed request's per-call pump publishes through the same channel |
| Redis | one per side, each awaiting a `PUBLISH`'s answer before the next | the client's `send`; the reply path and `refuse` on the server |
| RabbitMQ | one per side; the client's publishes in order and waits for each confirmation beside the publishes that follow | the client's `send` and the reply router's `opened`; the reply path and `opened` on the server |
| MQTT | rumqttc's event loop, through the client's order lock and the request channel | unchanged: the client's publishes and per-call pump hold the lock across each publish; the server's replies use the request channel, its `opened` and refusals `try_publish` from the event loop |
| Kafka | one per side; queues each record with librdkafka in order and waits for each delivery report beside the records that follow | the client's `send` and the reply router's `opened`; the reply path and `opened` on the server |

- **Order is the order of first polls.** Each send pushes its frame onto a tokio bounded `mpsc` of
  64 at its first poll and awaits the writer's answer through a oneshot, both of which poll
  without a runtime. tokio's semaphore gives permits in the order they were asked for, so a send
  waiting on a full queue keeps its place. A frame pushed before its future is dropped is still
  written: a request whose call timed out mid-send is followed by its `cancel` rather than lost, as
  on TCP.
- **The writer runs on the link's runtime,** spawned on its handle where `listen` or `connect`
  builds the side, so e1's rule holds: I/O on the link's runtime, results handed back. `Tokio::run`
  stays for `listen`, `connect`, the drains and the closes.
- **Confirmations and delivery reports wait beside the next publish.** RabbitMQ's writer awaits
  `basic_publish`, which lapin answers once the frames are written to its connection in call order
  (`lapin-4.12.0/src/channel.rs:486-520`), then pushes the `PublisherConfirm` into a
  `FuturesUnordered` the writer polls between jobs. Kafka's queues the record with `send_result`,
  retrying `QueueFull` every 100 ms for up to 5 s as `FutureProducer::send` does, and pushes the
  `DeliveryFuture`. Order is fixed when the frame is queued with the library; a writer awaiting each
  round trip before the next would cap a link at one publish per round trip. Redis has no split
  between queueing and answering a `PUBLISH` (the miss signal is its answer), so its writer awaits
  each in turn.
- **The brokers' held control frames are the writer's.** On RabbitMQ and Kafka the per-call pump
  task is gone: an `in` or `in_end` for a call whose `opened` has not arrived is held in the call's
  entry, the reply router sends `Job::Opened(call)` down the same queue, and the writer publishes
  the held frames before any job queued after it. A `cancel` before `opened` drops what was held,
  as the pump did (F376). NATS and MQTT keep their per-call pumps, which publish through the
  library's one channel.
- **Not frames:** RabbitMQ's `Ack` settles on a task of its own per delivery and Kafka's stores its
  offset in place, as before. An acknowledgement is created after the call's last reply has
  answered, so it follows that reply on the channel either way.

### 2. S4: the response's premise about data frames does not hold for the e1 code

The response states that one task per frame can reorder a call's `in` items and its `in_end`. In
the e1 code each frame's `send` was awaited before the next was created: `RpcClient`'s pump awaits
`conn.send(..)` per item (`crates/ulo-rpc/src/client.rs`, `pump`), the dispatch awaits each reply
(`crates/ulo-rpc/src/dispatch.rs:540`), and `Tokio::run` answers only once its task has ended. The
ordered client stream passed against Redis's link as at `HEAD` (the table below). What did reorder
was a frame sent while another frame's task was still running: a `cancel`, sent from the task a
dropped call spawns, against its request. The writer fixes both, and the ordered client stream
fails against a break that detaches each frame's task, the shape the response describes. See S1.

### 3. S4: where the `cancel` check applies

`cancel_follows_its_request` drives the links through `Link` directly, below `RpcClient`, which
offers no way to send a `cancel` right behind its request. It runs on TCP, one connection carrying
both, and on Redis, one publisher and one Pub/Sub connection carrying the request's channel and the
control channel in publish order. On NATS, RabbitMQ, MQTT and Kafka the `cancel` travels a control
lane of its own and the broker may deliver it first (U17), and UDP is unordered, so the check would
pass or fail by chance there; it is not stamped into the suite, whose `not_applicable` rule requires
a declared scenario to fail when run. See S2.

### 4. S1: `usable`, not `prepare`, on the client paths

- **The decision the brief asked for: a handle check, through a new `Link::usable`.** `prepare`
  takes `&mut self` and an `AppHandle`. `RpcClient::new` has no app. `RpcClientModule` holds its
  link in an `Arc` shared with the client it binds, and neither a module hook nor a factory can
  read an `AppHandle`: the injectables are `Dep`, `Many`, `Ext`, `ModuleRef` and `ExecutionRef`
  (`crates/ulo/src/dependency/handles.rs`). Each link's `prepare` checks a server's concerns too: the
  default group from the root module's path, Kafka's timer, TCP's endpoint resolved for listening.
  See S3.
- **`RpcClient::new` returns `Result<_, UnusableLink>`,** the shape of `timeout`'s `ZeroTimeout`,
  so `RpcClient::new(link, rt)?.timeout(b)?` refuses each mistake on its own line. Every caller
  changed: `ulo-rpc`'s `tests/runtime.rs` (four), TCP's `tests/handle.rs`, the conformance crate's
  plain-thread scenario, and the doc example.
- **The module calls `usable` from `on_init`,** the module's init hook, which fails `connect` as
  `ConnectError::Hook { reason: Errored(..) }` carrying an `UnusableLink`. `wire()` could refuse it
  sooner, as the zero timeout is refused; the response names the connect phase.
- **The links:** each of the seven answers `self.runtime().map(drop)`, the same refusal naming
  `.with_handle(..)`; each `prepare` calls `usable` where it called `runtime()`, TCP collecting it
  among its problems. `Tcp::new(addr).with_handle(h)` written off a runtime is still usable: the
  check runs when a client takes the link.

### 5. S9: `BroadcastAdapter::prepare` and both hubs

- **Synchronous, taking nothing,** since the hand-off's `UpgradeHandler::prepare` is synchronous.
  The default answers `Ok(())`; `InMemory` keeps it.
- **Both hubs call it through `Hub::prepare`:** the hand-off pushes the refusal onto its
  `Failures` beside `WsModule`'s defaults, and `GatewayTable::own_port`, which the standalone server
  calls, pushes it onto the server's, so `listen()` answers `Configure` naming `.with_handle(..)`.
  An app binding both servers reports it from each.
- **What became of the per-publish refusal.** `subscribe` without a runtime answers an empty stream
  and no longer logs: a hub subscribes only once `prepare` has passed. `publish` keeps its refusal,
  for the one path no `prepare` covers: a process that imports `WsModule` with the Redis adapter,
  serves no gateway and broadcasts through `Rooms`. Without a runtime the only alternative answer is
  a panic. See S4.

### 6. F369: the guard and its test

`connect` and `listen` hold the loop's `AbortHandle` in `AbortOnDrop` from the spawn until the
broker's answer, and disarm it once the `JoinHandle` is in the link's state; a failed answer drops
the guard, which aborts as `task.abort()` did. The test stands a socket in for the broker, since
only a broker that answers late shows the leak: without the guard a loop dropped before the CONNACK
receives it and goes on to SUBSCRIBE; with no CONNACK at all rumqttc's connect timeout, five seconds
unset (`rumqttc-0.25.1/src/v5/mod.rs:140`), would end the loop and hide the leak.

### 7. F370: the commit and its test

`close` runs the synchronous commit through `blocking` and awaits it. The test pauses the broker's
container after an offset is stored, polls `close` for 3 s on a current-thread runtime beside a
task ticking every 50 ms, then resumes the broker. The first form of the test resumed the broker
only after the closing thread ended; against the break that thread had not ended after 10 minutes
27 seconds, librdkafka holding a synchronous commit without bound while the broker is frozen, and
was killed, its paused container removed by hand. The test now resumes the broker a second after
the watch, so a commit on the closing thread ends then and the assertion reports it.

### 8. S10: the job's name

`rpc conformance (brokers)` is `broker integration`, and its id `conformance` is `brokers`; no job
`needs` it. The job's comment names its two kinds of test, and the `test` job's comment and the
Makefile's name the job. The artifact `conformance-failures-brokers` keeps its name.

## The tests

- **`client_stream_in_order`,** stamped into every suite: 256 items through `send_stream` to a new
  handler, `conformance.collect`, which answers what arrived before `in_end`; the answer must be
  `0..256`, and a short answer names how many items preceded `in_end`. UDP asserts the startup
  refusal, as every streamed scenario does there.
- **`cancel_follows_its_request`** (`crates/ulo-rpc-conformance/src/cases/order.rs`), called from
  `crates/ulo-rpc-tcp/tests/conformance.rs` and `crates/ulo-rpc-redis/tests/conformance.rs`: 200
  rounds, each starting a request's send and its `cancel`'s together with `join!`, then every
  request and every `cancel` read at the server's link, each `cancel` naming a call it holds.
- **S1:** `a_client_on_a_link_it_cannot_use_is_refused_where_it_is_built` and
  `a_client_module_on_a_link_it_cannot_use_fails_connect` in `crates/ulo-rpc/tests/runtime.rs`,
  over a link whose `usable` refuses and which counts its connects (none); on each of the seven
  links, `a_client_on_a_link_built_outside_a_runtime_and_given_none_is_refused_where_it_is_built`;
  on TCP also `a_client_module_on_a_link_built_outside_a_runtime_and_given_none_fails_connect`.
- **S9:** `a_broadcast_adapter_refusing_in_prepare_fails_the_http_server_s_listen`
  (`crates/ulo-ws/tests/handoff.rs`); in `crates/ulo-ws-redis/tests/handle.rs`, replacing the test of
  the per-publish refusal, `an_adapter_built_outside_a_runtime_and_given_none_refuses_in_prepare`
  (and one given a handle prepares) and
  `a_standalone_server_over_an_adapter_with_no_runtime_is_refused_at_listen`. The in-memory adapter's
  tests and the hand-off's nine others pass unchanged.
- **F369:** `a_connect_dropped_before_the_broker_answers_ends_its_event_loop` and
  `a_listen_dropped_before_the_broker_answers_ends_its_event_loop`
  (`crates/ulo-rpc-mqtt/tests/abandoned_connect.rs`), no broker needed.
- **F370:** `close_commits_off_the_runtime_thread_polling_it` (`crates/ulo-rpc-kafka/tests/conformance.rs`,
  under `integration`), requiring at least 20 ticks.

### Before and after

Each break went through `batch22/scripts/brk.py`: it replaces each span (asserting it occurs once)
or restores a file to `HEAD`, runs the target, writes every touched file and `Cargo.lock` back byte
for byte, and compares a hash over `git diff HEAD` of `crates`, `Cargo.toml`, `Cargo.lock` and
`.github` and every untracked file under `crates` and `.github`. Every restore reported `ok`, the
hash unchanged; source edits made between breaks move it, and the last thirteen read `070d68827346`.
Specs are in `batch22/specs/`, full output in `batch22/broken/`.

| Break | Target | Result |
| --- | --- | --- |
| `crates/ulo-rpc-redis/src/link.rs` as at `HEAD`, a `Tokio::run` task per frame | the Redis suite's `cancel_follows_its_request` and `client_stream_in_order` | the check failed, "200 requests and 136 cancels of 200 each reached the server"; the ordered stream passed |
| Redis's client writer spawning a detached task per frame, each sleeping 0–3 ms first and answering at once | same | both failed: the server received 90 of 256 items before `in_end`, out of order from index 1; 97 cancels of 200 |
| RabbitMQ's writer publishing a call's held frames in reverse at `opened` | RabbitMQ's `client_stream_in_order`, `client_stream`, `bidi_stream` | all three failed; the ordered stream's items began 79, 78, 77, …, those held before `opened` |
| Kafka's the same | Kafka's same three | all three failed; `in_end`, held with the items, went first: 0 of 256 items |
| `connect`'s guard never armed | `abandoned_connect` | the connect test failed: the dropped loop sent SUBSCRIBE |
| `listen`'s guard never armed | same | the listen test failed the same way |
| `crates/ulo-rpc-mqtt/src/link.rs` as at `HEAD` | same | both failed the same way |
| Kafka's close commit on the calling thread, the test's first form | `close_commits_off_the_runtime_thread_polling_it` | blocked 10 min 27 s and was killed; its paused container removed by hand |
| the same, the test's final form | same | failed, "ran its other task 0 times in 4.014006541s with the broker frozen" |
| `RpcClient::new` without the `usable` check | `ulo-rpc --test runtime`; each link's `handle` | the `RpcClient::new` test failed in `ulo-rpc` and on all seven links |
| the module's init hook answering `Ok` | `ulo-rpc --test runtime`; TCP's `handle` | the module test failed in each |
| TCP's `usable` left at the default | TCP's `handle` | three failed: the client, the module, and the server's `prepare` |
| the hand-off not calling the adapter's `prepare` | `ulo-ws --test handoff` | the new test failed, the HTTP server listening |
| `own_port` not calling it | `ulo-ws-redis --test handle` | the standalone test failed, the server listening |
| the Redis adapter's `prepare` left at the default | same | both failed |
| a link to a missing item in `Link::usable`'s doc | `RUSTDOCFLAGS="-D warnings" cargo doc -p ulo-rpc` | error, "unresolved link to `NoSuchItem`" |
| an unused import planted in UDP's link | `scripts/diag.py` over `cargo check` | reported, the one location outside `crates/ulo/src` |

## Left for the transports DESIGN fold

Line numbers are those read on `269afe50`.

- §5.3's `Link` block (lines 915-935): `fn usable(&self) -> Result<(), BoxError>`, defaulted, beside
  `prepare`, whose comment gains that it reports `usable`'s refusal.
- §5.3, "Every link is tokio-based" (line 983): a link with no runtime is refused by `usable` where
  a client takes it, by `RpcClient::new` as `UnusableLink` and by `RpcClientModule`'s init hook as
  a failed `connect`, not at the first call; "On UDP, Redis, RabbitMQ and Kafka the client's `send`
  and the server's reply path run through `run`, so each frame costs a task …" becomes the writer
  rule of decision 1, with confirmations and delivery reports waited on beside later publishes, the
  brokers' held control frames released by the writer on `opened`, and the one remaining race U17's;
  the MQTT exception (F369) is gone; Kafka's close commit is among the blocking calls on threads of
  their own.
- §5.3 or §5.2's frame paragraph: frames from one link go out in the order their sends were first
  polled, a streamed request's held control frames excepted, which keep their own order (the
  `Outbound` doc).
- §5.4's client paragraph (line 1024): `RpcClient::new(link, runtime)` returns `Result<RpcClient,
  UnusableLink>`; "A link built outside a runtime and given none fails the first call `Unavailable`
  …" is replaced by the refusal at construction and at `connect`.
- §5.2's conformance paragraph and scenario list (lines 905, 909): `client_stream_in_order`; the
  link-level `cancel_follows_its_request`, called by TCP's and Redis's suites; "CI's broker job" is
  `broker integration`.
- §4.3 (line 831): `BroadcastAdapter::prepare`, defaulted, called by both hubs' `prepare`; line 837
  and §1's `fw-ws-redis` row (line 34): with no runtime the adapter fails `prepare` and a server
  serving gateways refuses `listen()`; a publish without one still fails, for a process serving
  no gateway.
- §8's `close` row (line 1282): Kafka's offsets committed on a thread of its own.
- §10's failure table (line 1382): a client's link with no runtime fails at construction or
  `connect`; `fw_ws_redis::Redis` with none fails `listen()` where a server serves gateways.
- §11, X29 (line 1319): `RpcClient::new` returns a `Result` with `UnusableLink`; a row for
  `Link::usable` and `BroadcastAdapter::prepare`.
- Decision 64 (line 1485): the refusal reaches a client where it is built rather than "as the first
  call's `Unavailable`".
- Decision 71 (line 1492) is superseded by the forty-first response's S9 as decision 5 builds it.
- Decision 72 (line 1493): the job is renamed `broker integration`.

## Needs sign-off

Numbered from S1 for this batch; the items it builds are named by their logs.

### S1. The ordered client stream did not fail against the e1 code

Decision 2. The response's premise, data frames of one call reordering under a task per frame, does
not hold for e1, where each frame's send finished before the next was made; the scenario guards
against a send that answers before its frame is queued, and fails against a break that detaches
each frame's task. What e1 did reorder was a `cancel` against its request, which the check of S2
shows. The alternative is a scenario widened to reach e1's code itself, which no sequence of awaited
sends can.

### S2. The `cancel` check is a link-level test on TCP and Redis, not a stamped scenario

Decision 3. `RpcClient` cannot send a `cancel` right behind its request, and on four brokers and UDP
the order is not observable. The alternatives are a capability declaring that a link keeps a
request and its `cancel` in order, asserted where declared, or the check stamped with
`not_applicable` on five links, against the rule that a declared scenario fails when run.

### S3. The client paths check `usable`, not `prepare`

Decision 4. A module cannot reach an `AppHandle` and `RpcClient::new` has none, and `prepare` checks
a server's concerns. The alternatives are a core change handing a module's hooks the app, or
`prepare` split into a server half and a client half per link.

### S4. The Redis adapter's `publish` keeps its refusal without a runtime

Decision 5. A process serving no gateway never prepares the adapter, and a panic is the only other
answer; `subscribe`'s log is gone. The alternative is `WsModule` calling the adapter's `prepare`
from its own init hook, which fails `connect` for every app importing it, ahead of the `listen()`
refusal the response describes.

### S5. A frame pushed onto a writer's queue is written after its send is dropped

Decision 1. A request whose call timed out mid-send now reaches the server, followed by its
`cancel`, where e1's dropped `Tokio::run` aborted an unstarted send. TCP behaved this way already.
The alternative is a send that removes its frame from the queue when dropped, which a tokio `mpsc`
cannot do.

### S6. RabbitMQ and Kafka wait for confirmations beside later publishes; Redis awaits each `PUBLISH`

Decision 1. The alternative for RabbitMQ and Kafka is awaiting each confirmation or delivery report
in turn, one publish per round trip per link; for Redis, pipelining, which needs the miss signal
split from the publish.

### S7. The job's id is renamed with its name

Decision 8. The alternative is keeping the id `conformance`, which names neither kind of test.

## Verification

Full, unfiltered output of every run is in the session scratchpad, `batch22/`: `suites/` (with
`summary.txt`), `verify/` (with `summary.txt`, `crates-summary.txt` and `tree.txt`), `broken/`,
`specs/`, `runs/` and `scripts/`.

- **Conformance**, three consecutive runs per suite, one crate at a time, the brokers with
  `--features integration --test conformance --locked`, test time as libtest reports it
  (`suites/summary.txt`):

| Suite | Run 1 | Run 2 | Run 3 | Result |
| --- | --- | --- | --- | --- |
| `ulo-rpc-tcp` conformance | 1.32 s | 1.32 s | 1.31 s | 29 passed: the suite's 28 and the `cancel` check |
| `ulo-rpc-tcp` conformance_cbor | 1.31 s | 1.31 s | 1.31 s | 28 passed |
| `ulo-rpc-tcp` conformance_tls | 1.32 s | 1.31 s | 1.32 s | 28 passed |
| `ulo-rpc-udp` conformance | 1.31 s | 1.31 s | 1.31 s | 27 passed, 1 ignored |
| `ulo-rpc-nats` | 3.61 s | 3.07 s | 2.86 s | 28 passed |
| `ulo-rpc-redis` | 5.76 s | 5.38 s | 5.31 s | 30 passed: the suite's 28 and two link tests |
| `ulo-rpc-mqtt` | 5.87 s | 5.57 s | 5.53 s | 33 passed: the suite's 28 and five link tests |
| `ulo-rpc-rabbitmq`, `ULO_CONFORMANCE_PARALLEL=6` | 19.61 s | 19.96 s | 20.31 s | 28 passed |
| `ulo-rpc-kafka` | 25.42 s | 25.28 s | 24.15 s | 29 passed: the suite's 28 and the F370 test |
| `ulo-ws-redis` `--test integration` | 0.49 s | 0.33 s | 0.67 s | 1 passed, `peer_instance` ignored |

  The sources did not change after these runs.
- **Each crate's tests,** `cargo test -p <crate> --locked` with the OpenSSL flags
  (`verify/crates-summary.txt`): `ulo-rpc` 19 passed and 5 ignored; `ulo-ws` 50 and 4; `ulo-ws-hyper`
  70 and 1; `ulo-ws-redis` 2 and 1; `ulo-tokio` 34 and 1; `ulo-rpc-conformance` 2 and 2;
  `ulo-rpc-tcp` 95 and 1; `ulo-rpc-udp` 32 and 2; `ulo-rpc-nats`, `-redis` and `-rabbitmq` 2 and 1
  each, their `handle` tests; `ulo-rpc-mqtt` 4 and 1, `abandoned_connect` among them;
  `ulo-rpc-kafka` 5 and 1. Every run exited 0.
- `cargo test --workspace --no-fail-fast` with the OpenSSL flags: 653 passed, 0 failed, 72 ignored
  across 166 test binaries (`verify/workspace-test.txt`): batch 21's 634 and nineteen added, the
  ordered client stream on TCP's three suites and UDP's (4), TCP's `cancel` check, `ulo-rpc`'s two
  `UnusableLink` tests, the seven links' client refusals and TCP's module refusal (8), MQTT's two
  F369 tests, the hand-off's adapter test, and `ulo-ws-redis`'s `handle` at two where it held one.
  The binary added is `abandoned_connect`.
- `cargo check --workspace --all-targets --all-features` with
  `CFLAGS=-I/opt/homebrew/opt/openssl/include LDFLAGS=-L/opt/homebrew/opt/openssl/lib`, and with
  defaults: exit 0, the 17 known warnings in `crates/ulo/src` and no other location
  (`scripts/diag.py`, shown against a planted warning in the breaks table).
- `cargo +1.88 check --workspace --all-targets --exclude ulo-http-salvo --exclude
  ulo-graphql-async-graphql`: exit 0, the same 17 and no other.
- **The tree checks,** `cargo tree -p <crate> -e normal -i tokio --locked` (`verify/tree.txt`):
  stdout is empty for `ulo`, `ulo-transport`, `ulo-net`, `ulo-http`, `ulo-rpc`, `ulo-ws`,
  `ulo-graphql-ws`, `ulo-graphql-http`, `ulo-ws-conformance` and `ulo-smol`, each exiting 101 with
  "did not match any packages" or 0 with "nothing to print"; `ulo-rpc-tcp` prints tokio, the
  positive control.
- `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --lib -p <crate>` for `ulo-rpc`,
  `ulo-rpc-conformance`, the seven links, `ulo-ws`, `ulo-ws-hyper` and `ulo-ws-redis`: each exits 0;
  the planted link failed it.
- `cargo +1.98.1 clippy` over the same twelve crates with `--all-targets --no-deps --locked` and the
  five broker links' and `ulo-ws-redis`'s `integration` features: exit 0, 16 warning locations
  outside `crates/ulo/src`, none on a line this batch changed or in a new file, by
  `scripts/changed_lines.py` over `git diff -U0 HEAD` and the untracked files; given an existing
  location as a planted changed line, it reported it (`verify/clippy.json`).
- **Containers:** the user's four, seaweedfs, mailpit, postgres:18 and redis:7, untouched and the
  only ones running after every suite run (`suites/summary.txt` counts 4 after each) and after each
  break. One container this batch started was left by a killed test, the Kafka broker of the F370
  break's first form, paused; it was resumed and removed with `docker rm -f`. No permission check
  refused an action, and the Docker engine answered throughout.
