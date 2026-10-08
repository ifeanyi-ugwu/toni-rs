# Divergences: race 2b, tests batch 17, the drain waits for what reaches it (F352), MQTT's drain waits for the UNSUBACKs (F351), NATS declares an unconfirmed drain (F331), TCP and UDP end their stream once idle (F353, F356), batch 14's two doc sentences

The thirty-second response, signed off 2026-10-08, settled four builds. The RPC server's drain
now covers what the link hands over during it and opens no execution: it waits, under the one
drain deadline, for the link's inbound stream to end and for every refusal spawned to be sent, and
`close` logs what the deadline cut off. That wait made TCP's and UDP's drains run to their
deadline with nothing pending, their inbound streams ending only when every connection closes or
at `close`; on the coordinator's ruling that this is a defect, both links now end the stream once
the draining server holds no call and answer later requests themselves, and TCP's `close` no
longer leaves an aborted connection's socket open. MQTT's drain returns once the broker has
acknowledged every UNSUBSCRIBE. The NATS probe the response asked for does not hold: async-nats
0.46 gives no public path that confirms a drain, so the NATS link declares
`Capabilities::unconfirmed_drain`, which a new conformance scenario asserts in both directions;
that scenario then found UDP losing a call sent as its server closed, and UDP declares it too. The
two sentences batch 14's sign-off asked for are in `ulo-http`'s docs. Redis, RabbitMQ and Kafka
were read and changed in nothing. F354 and F355 are filed open; F353 and F356 are filed and fixed.

Files changed: `crates/ulo/src/transport/server.rs` (doc only); `crates/ulo-rpc/{Cargo.toml,
src/dispatch.rs, src/server.rs, src/link.rs}`, `crates/ulo-rpc/tests/drain_window.rs` (new);
`crates/ulo-rpc-mqtt/src/link.rs`, `crates/ulo-rpc-mqtt/tests/conformance.rs`;
`crates/ulo-rpc-nats/src/{lib.rs, link.rs}`; `crates/ulo-rpc-tcp/{Cargo.toml, src/link.rs}`,
`crates/ulo-rpc-tcp/tests/drain_end.rs` (new); `crates/ulo-rpc-udp/{Cargo.toml, src/lib.rs,
src/link.rs}`, `crates/ulo-rpc-udp/tests/drain_end.rs` (new);
`crates/ulo-rpc-conformance/src/{lib.rs, cases/drain.rs}`; `crates/ulo-http/src/{body.rs,
service.rs}` (doc only); `Cargo.lock`; F331, F351, F352 and F353–F356 in the workspace's
`FRAMEWORK_GAPS.md`. The tree is `5ffbb554` plus this batch.

## The signatures

```rust
// ulo_rpc::Capabilities (new field and setter)
pub unconfirmed_drain: bool,
pub const fn unconfirmed_drain(self, unconfirmed_drain: bool) -> Self;
```

`Capabilities::new` sets it `false`; `ulo_rpc_nats::Nats` and `ulo_rpc_udp::Udp` declare it
`true`. Every other public
item is unchanged. Private to `ulo-rpc`: `dispatch::Settling` (`reading`, `ended`, `refusals`,
`settled()`), the `Shared::settling` watch, `dispatch::refuse` and its `Refusing` guard in place of
`send_error`, and in `server.rs` the `Reading` guard, `unread` and `abandon`. Private to
`ulo-rpc-mqtt`: `Unsubscribing`, `ServerSide::{unsubscribing, unconfirmed}` and the `Unconfirmed`
guard. Private to `ulo-rpc-tcp`: `State::held`, `Ids::new`/`counted`, `forward`, `idle`, `refuse`;
to `ulo-rpc-udp`: `State::{draining, held}`, `Ids::new`/`counted`, `idle`, `refuse`. `ulo-rpc`
gains the dev-dependencies `ulo-tokio` and tokio's `time` feature; `ulo-transport` becomes a
dependency of `ulo-rpc-tcp` (it was a dev-dependency) and of `ulo-rpc-udp`.

## Decisions

### 1. F352: the drain window covers the inbound stream and the refusals

- **Where the wait lives:** in `Server::drain`, after `Link::drain` returns
  (`crates/ulo-rpc/src/server.rs:235-246`). The core already awaits each server's drain future
  inside the drain window and drops it at the window's deadline
  (`crates/ulo/src/lifecycle/shutdown.rs:240-253`), so the wait takes that deadline and no bound or
  knob of its own. `shutdown.rs` is unchanged; the core's `Server::drain` doc now says the window
  waits for the future and drops it at the deadline, and that a server answering without an
  execution waits there for those answers.
- **What it waits for:** `Settling::settled`, `!reading || (ended && refusals == 0)`. `serve` sets
  `reading` when it takes the inbound stream, through a guard that clears it when `serve` returns
  or is dropped, so a server that never served, or whose serve future was dropped, waits for
  nothing. `ended` is set when the stream returns `None`. `refusals` counts refusal tasks from
  spawn to the end or abort of their send: the dispatcher's three refusal sites (over the
  in-flight limit, a call during the drain, an unhandled pattern during the drain) go through
  `dispatch::refuse`. Shed refusals spawned before the drain are counted too; waiting for one
  costs nothing the window does not already allow.
- **Order:** the serve loop routes deliveries in order, and `refuse` increments the count
  synchronously in `accept`, so every refusal for a delivery the stream yielded is counted before
  `ended` is set.
- **At close:** the closing branch of `serve` takes, without waiting, every delivery the inbound
  stream has ready and drops it, then reads the refusal count, and logs at `warn` when either is
  nonzero: "the RPC server closed before answering every call that reached it; the drain's deadline
  passed first", fields `link`, `refusals_unsent`, `deliveries_unread`. It then aborts the tasks as
  before. A dropped delivery's `Ack` settles nothing, so AMQP and Kafka redeliver it. A call still
  running at close was cancelled at the drain's end and is already counted in
  `Shutdown::abandoned`; it is not counted again. See S1.
- **Links that did not end their stream at the drain:** TCP's stream ended only when every
  connection had closed, and UDP's only at `close`, so with this wait their drain ran to its
  deadline whenever a TCP client stayed connected, and always on UDP. Decision 6.

### 2. F351: MQTT's drain waits for the UNSUBACKs

- **Mapping a filter to its UNSUBACK:** `Mqtt::drain` records each `$share` filter in
  `Unsubscribing::queued` before it queues the UNSUBSCRIBE; the event loop moves the oldest queued
  filter to `sent` under the packet id `Outgoing::Unsubscribe` reports, and removes it on the
  UNSUBACK with that id (`crates/ulo-rpc-mqtt/src/link.rs:220-248`, `:656-657`). rumqttc takes
  requests in the order they were queued and the server side unsubscribes nowhere else, so the
  order identifies the filter. A request rumqttc refuses to queue is taken back off `queued`.
- **Return:** once no filter is outstanding. The broker writes every PUBLISH it routed before
  processing an UNSUBSCRIBE ahead of the UNSUBACK and the event loop reads in order, so what was
  routed here has reached `on_publish` by then. The inbound stream's watcher is spawned only
  after, so a request routed before the UNSUBACK reaches the core and is refused by the
  dispatcher rather than by the link.
- **An UNSUBACK refusing a filter** (anything but Success or No Subscription Existed) is logged at
  `warn` naming the filter and reason, and counts as answered: the broker will go on routing, and
  nothing before close changes that.
- **A reconnect during the drain:** a CONNACK without a session present releases every
  outstanding filter; the link connects with a clean start, the new session holds no shared
  subscription, and the loop resubscribes only the control topic while draining.
- **No timeout in the link.** The core drops the future at the drain's deadline. The log fires
  from a guard's drop inside `Mqtt::drain`'s future, `Unconfirmed`, which names the filters still
  outstanding: "the drain's deadline passed before the MQTT broker confirmed the unsubscribe;
  until it does it can route requests to this instance", field `filters`. The core's deadline path
  was the other candidate; it has no logger (`ulo` depends on no `tracing`) and does not know the
  filters. The guard's drop is reached only when the future is dropped before confirmation, which
  in the sequence is the drain deadline or the shutdown cap. See S3.
- **What remains:** MQTT 5 §3.10.4 lets a broker deliver, after the UNSUBACK, messages it had
  buffered for the client. Filed as F355.

### 3. NATS: the probe does not hold; `unconfirmed_drain`

**Reading async-nats 0.46.0.** `Client::flush` sends `Command::Flush`, whose observer resolves
once the connection's `poll_flush` returns, the client's own write (`src/lib.rs:633-646`,
`src/client.rs:709-719`); no PING is sent and no PONG awaited, so the response's premise does not
hold for this client. `Subscriber::drain` queues `UNSUB` and `PING` and pushes the sid onto
`drain_pings` (`src/lib.rs:830-849`); the next poll of the connection task removes it from the
subscription table after reading what is ready (`src/lib.rs:559-564`). `Subscriber::unsubscribe`
removes it at once (`src/lib.rs:809-825`). A `MSG` for a sid not in the table is discarded
(`src/lib.rs:709-783`). `Command`, `ClientOp` and the client's sender are `pub(crate)`
(`src/lib.rs:375`, `:405`, `src/client.rs:83`). `unsubscribe_after(n)` keeps a subscription until
`n` deliveries, which is not a drain. No public path unsubscribes, waits for the server, and still
delivers what arrives before it has processed the `UNSUB`.

**The probe.** A temporary test, `crates/ulo-rpc-nats/tests/f331_probe.rs`, with a temporary edit
in the link selecting the drain variant and temporary dev-dependencies, all removed. A
`nats:2.10.14` container; the RPC server's link reaches it through a relay that, once armed, holds
every byte the link writes for 500 ms, while the NATS server's writes to the link pass at once. A
raw async-nats client warms up with one request, the relay is armed, the app's close starts, and
20 ms after `is_draining()` the client sends a request, which the NATS server routes to the
draining instance before the `UNSUB` reaches it. With noise, one message to the control subject
goes 20 ms before the request, standing for any other traffic on the link's connection. The
request waits three seconds.

| Variant | Noise | Outcome |
| --- | --- | --- |
| A: `Subscriber::drain` (the link as built) | none | answered after 527 ms: `{"t":"err","id":2,"e":{"kind":"unavailable","message":"the server is draining","details":[]}}` |
| A | one message | no answer after 3.05 s: `request timed out: deadline has elapsed (TimedOut)` |
| B: `unsubscribe`, then `flush` (the response's proposal) | none | no answer after 3.03 s: `TimedOut` |
| B | one message | no answer after 3.05 s: `TimedOut` |
| C: `drain`, then an echo through the connection's own inbox | none | answered after 1.00 s: the same `unavailable` refusal |
| C | one message | no answer after 3.05 s: `TimedOut` |

Each run's full output is in the scratchpad (`runs/f331-<variant>-noise<0|1>.txt`). Where the
request was answered, nothing woke the connection task between the `UNSUB` write and the
request's arrival, so the poll that read the `MSG` found the sid still in the table and removed it
afterwards; one message arriving first, or the `Flush` or `Subscribe` command of the variant itself
in B, gives the poll that removes the sid before the request arrives. A real application has many
pattern subscriptions draining on one connection, each drain a command, and replies and control
messages on the same connection. Unsubscribe-then-flush makes the loss certain rather than closing
the window.

**The capability.** `Capabilities::unconfirmed_drain`: the link's drain cannot confirm that the
broker has stopped routing to the instance, and a request routed before it stopped can be lost on
its way in. The name states what the link cannot do, as `holds_unserved` and `durable_replies`
state what the broker does, and the field's doc names what a caller sees, its own `Timeout`, as
theirs do. A name for the effect alone (`lossy_drain`) would read as a defect of every call during
the drain rather than of the window before the broker processes it. NATS declares it
(`crates/ulo-rpc-nats/src/link.rs:93`); its crate doc and the request lane's comment say why.

**UDP declares it too.** The first verification pass ran `drain_window` on UDP and failed once in
three: "call 411 of 412 sent during the drain was not refused `Unavailable`, got: Err(Timeout)"
(`verify-before-udp-capability/udp-conformance-1.txt` in the scratchpad). UDP's drain then ran to its
deadline (F353, fixed by decision 6), the scenario sent calls throughout, and the last one left as the server closed:
nothing tells a UDP caller to stop sending, and a datagram still in the server's socket when it
closes is dropped, with no ICMP unreachable to report it. That is a drain the link cannot confirm,
so the field's doc now names both links: NATS for a broker it cannot hear stop routing, UDP for
callers it cannot tell to stop. Ending UDP's stream once idle (decision 6) narrows the window to the gap
between the receive task stopping and the socket closing and does not remove it. See S6.

**The assertion.** A new scenario, `drain_window` (`crates/ulo-rpc-conformance/src/cases/drain.rs:87`),
stamped beside `drain` in `conformance_suite!`. The server holds no call, so its drain is as short
as the link makes it. The scenario starts the close, polls `is_draining()` with no sleep, and sends
calls back to back, 10 ms apart, until the close has finished, then one more. Each call during the
drain must be `Unavailable`; the caller's `Timeout` passes only on a link without `miss_signal`,
declaring `holds_unserved`, or declaring `unconfirmed_drain`. The call after the close must be
`Unavailable` unless the link lacks `miss_signal` or declares `holds_unserved`, so on NATS it shows
the drain did reach the broker. A link not declaring the capability is held to the confirmed
drain, a declaring one to what it promises. The existing `drain` scenario is unchanged and still
requires `Unavailable` from NATS for a call sent `settle / 2` into the drain. See S4.

### 4. Redis, RabbitMQ and Kafka: whether each drain is confirmed

Read, not changed.

- **Redis: confirmed.** `drain` awaits `PubSubSink::unsubscribe(&patterns)`
  (`crates/ulo-rpc-redis/src/link.rs:125`), which sends one `UNSUBSCRIBE` and awaits the reply
  (`redis-1.7.1/src/aio/pubsub.rs:314-317`, `send_recv` at `:270-284`). The pipeline hands every
  pushed message to the `PubSubStream` before it resolves the waiting request
  (`handle_message`, `:112-158`). Redis runs the whole command before replying, so the first
  channel's reply already follows the unsubscribe of every channel; redis-rs resolves on that first
  reply and pops nothing for the others, harmless here since nothing else is in flight on the sink.
  The gap is downstream: the watcher ends the inbound stream as soon as no call is held
  (`link.rs:129-134`), and a request the server lane reads after that is held and dropped
  (`link.rs:229-233`, `:265`). Filed as F354.
- **RabbitMQ: confirmed.** `basic_cancel` with `nowait: false` (`crates/ulo-rpc-rabbitmq/src/link.rs:181`)
  awaits `basic.cancel-ok` (`lapin-4.12.0/src/generated/channel.rs:568-602`). Until then the
  consumer stays registered as `Canceling` and still receives deliveries
  (`src/channel.rs:624-626`, `src/consumers.rs:36-55`); the cancel-ok deregisters it and ends its
  stream (`src/channel.rs:1269-1272`), and the broker sends cancel-ok after every delivery to that
  consumer. A delivery read after the watcher ended the inbound stream (`link.rs:186-190`) is
  dropped with its `Ack` unsettled, and AMQP requeues it at channel close, which `holds_unserved`
  declares.
- **Kafka: nothing to confirm.** A consumer group member pulls; no broker routes to it. "Stop
  accepting" is `pause` on the assignment (`crates/ulo-rpc-kafka/src/link.rs:242`), local to
  librdkafka: it bumps the partition's version barrier before returning
  (`rdkafka-sys-4.10.0+2.12.1/librdkafka/src/rdkafka_partition.c:2400`), and the consumer queue
  drops prefetched messages of an older version (`rdkafka_queue.c:363`). The partitions stay
  assigned until the consumer leaves the group at close. Offsets are stored only when a handler
  settles (`enable.auto.offset.store=false`, `link.rs:174`; `store_offset` at `:460`), and the
  drain's and close's commits commit stored offsets, so a request never handed over stays in the
  topic and the next member resumes from it after the rebalance: `holds_unserved`. A record polled
  before the pause and dropped after the inbound stream ended is unsettled and redelivered the same
  way, unless a later offset on its partition is stored first; offsets are cumulative per
  partition.

TCP and UDP are outside this: no broker routes to them, so nothing stands between their drain and
the end of deliveries but their own listener and socket.

### 5. Batch 14's two sentences in `ulo-http`

- **S2's:** on `HttpBody::stream` (`crates/ulo-http/src/body.rs:36-40`) and in step 8 of
  `AppService`'s doc (`crates/ulo-http/src/service.rs:65-77`): `on_stream_end` reports the reply
  actually written, and a stream discarded before the response is sent is not a reply and reports
  nothing. Not on `Execution::on_stream_end` in `crates/ulo`: on RPC a `Reply::Many` the error
  handlers answer after the deadline is dropped unwritten, and its `Tracked` reports
  `CutOff(Deadline)` from its drop (`crates/ulo-rpc/src/dispatch.rs`, `answer`;
  `crates/ulo-transport/src/tracked.rs:28-36`), so the sentence holds on HTTP alone. See S5.
- **S4's:** in step 8 of `AppService`'s doc, where bodiless answers are documented: such a stream
  reports `Completed` when the service answers, since the outcome concerns the body and such a
  response owes none, and a peer leaving before the head is written is a connection failure, not an
  incomplete body. The response's further clause, that such a peer is reported as a disconnect where
  the transport observes it, was not written: batch 14 marks the body ended, so its drop fires no
  `Disconnected`, and nothing else reports one.

### 6. TCP and UDP end their inbound stream once idle (F353); TCP's `close` releases aborted connections (F356)

The first verification pass ran the TCP suite in 15.52 s for 25 scenarios, against batch 14's
1.31 s for 24. The difference is the drain, not load: every scenario run alone took 5.04–6.10 s
(`runs/tcp-per-scenario-fixed.txt`), the conformance app's five-second drain window plus each
scenario's own length, and the suite took 15.52–15.53 s in all twelve TCP runs of the first two
passes at load averages from 3.4 to 7; `drain` and `drain_window`, whose clients had left their
connection after `goaway`, took 1.34 s and 0.05 s. The servers stopped while their client was
still connected, held no call, and waited out the window. The response's rule was to report a
link breaking the promise; the coordinator ruled a drain waiting out its deadline with nothing
pending a defect, and the links were fixed.

- **TCP:** every connection's deliveries now pass through one forwarder task (`forward`), which
  hands them to the server until the link is draining and holds no call, then ends the inbound
  stream and answers what arrives afterwards: a request or streamed request `err` of kind
  `unavailable`, "the server is draining", sent on its reply path, which also forgets its id;
  anything else names no call and is dropped. Calls are counted across connections
  (`State::held`, kept by `Ids`). A `cancel` forgets its id before it is queued, and a closing
  connection forgets its orphans before their `cancel`s are queued, so both are released only
  after the queueing; the forwarder reads its queue before it looks at the count (`biased`), so
  nothing queued before the count reached zero is answered by the link instead of the server.
- **UDP:** the receive loop is the only sender and decides the same between datagrams, refusing a
  later request from the socket under the sender's own id until `close`. `drain` raises a flag in
  place of doing nothing.
- **F356, found by `drain_window`:** with the drain now ending at once, a TCP call sent as the
  server closed waited for its own `Timeout` in 4 of 6 runs. `close` aborts the connection tasks,
  and an aborted task never removes its writer's queue sender from `State::connections`, so the
  writer never ended and the socket stayed open with nothing reading it. `close` now clears the map
  once the accept task has ended; each writer then shuts its half, and the caller's reply lane ends
  `Unavailable`.
- Every TCP scenario alone now takes its own length again, 0.03–1.34 s
  (`runs/tcp-per-scenario-idle-end.txt`), and UDP's 0.02–1.33 s.

## The tests

- **`crates/ulo-rpc/tests/drain_window.rs`, three tests,** over `Scripted`, a link whose inbound
  stream the test feeds and ends, whose `drain` returns at once, and whose `close` can hand over
  deliveries after the server raised its close signal. Current-thread runtimes;
  `Running::window` waits until the app stops serving or 50 ms pass, so a server whose drain waits
  for nothing has closed before the call is handed over.
  - `a_call_the_link_hands_over_after_its_drain_returned_is_refused_before_close`: a `req`
    delivered after `drain` returned, then the stream ended; the reply path must receive `err` of
    kind `unavailable`.
  - `a_refusal_spawned_before_the_inbound_stream_ended_is_sent_before_close`: a `req` delivered and
    the stream ended at once, the reply path's send held on a gate opened after the window; the
    refusal must be received.
  - `the_drains_deadline_logs_what_close_leaves_unanswered`: a 200 ms drain window, a refusal held
    on a gate never opened, a stream that never ends, two deliveries handed over by `close`; nothing
    may be answered and the `warn` must read `refusals_unsent=1 deliveries_unread=2`.
  - The capture is the process's global subscriber, its lines keyed by thread. The first version
    used a thread-local default subscriber, and the deadline test failed under one undo with no
    `warn` captured while it passed alone: tracing caches a callsite's interest from the first
    thread to reach it, and with one dispatcher registered in the process it asks that thread's
    default (`tracing-core-0.1.36/src/callsite.rs:544-570`), so the refusal-wait test reaching the
    `warn` first, with no subscriber, disabled it for the test that had one.
- **`crates/ulo-rpc-mqtt/tests/conformance.rs`, two tests** beside batch 16's, each against its
  own Mosquitto container.
  - `drain_returns_once_the_broker_stops_routing_to_the_server`: after the server link's `drain`
    returns, a request published by a client link must be answered `err` `unavailable` with
    `reason: "no_destination"`, the broker's PUBACK 0x10, and the server must not receive it.
  - `a_drain_dropped_before_the_broker_confirms_logs_the_filters`: on a current-thread runtime the
    drain future is polled once and dropped, before the event loop could write its UNSUBSCRIBE; the
    `warn` must name `$share/drain_unconfirmed/link.unconfirmed`. The capture is the same global,
    thread-keyed subscriber.
- **`drain_window`** in the shared suite, decision 3, run by every link's conformance target.
- **`crates/ulo-rpc-{tcp,udp}/tests/drain_end.rs`, one test each,**
  `a_server_holding_no_call_stops_without_waiting_out_its_drain`: a server with a 30 s drain
  window, a client that made one call and stays connected, and the server's close required to
  finish within 5 s.

### Before and after

Each fix was undone through a script that rewrote one line, ran the target and wrote the file back
byte for byte; the full output of every run is kept in the scratchpad (`runs/`).

| Code | Probe | Result |
| --- | --- | --- |
| `drain_window.rs` against `5ffbb554`'s `dispatch.rs` and `server.rs` | none | 3 failed: "the call handed over during the drain was not refused `unavailable`: Err(Disconnected)", "the refusal spawned during the drain was not sent: Err(Disconnected)", "no `warn` counting what close left unanswered: []" |
| fix, three runs | none | 3 passed each, 0.20 s |
| fix, `Server::drain`'s wait rewritten to `let _ = shared;` | none | 3 failed; the deadline test's `warn` read `refusals_unsent=0`, the refusal not yet spawned when `close` ran |
| fix, `settled` rewritten to `!self.reading \|\| self.ended` | none | the refusal test failed; the other two passed |
| fix, the `warn`'s condition rewritten to `false && ..` | none | the deadline test failed: `[]`; the other two passed |
| MQTT tests against `5ffbb554`'s `link.rs` | none | the deadline-log test failed; the confirmation test passed, the UNSUBSCRIBE reaching the broker first |
| the same | server event loop 15 ms behind its queue | both failed; the request published after `drain` returned was routed to the draining server: `Some(Err { id: 1, error: ErrorBody { kind: Unavailable, message: "the server is draining", details: Details([]) } })` |
| fix | 15 ms behind | both passed |
| fix, the drain's wait rewritten to `let _ = &side.unconfirmed;` | 15 ms behind | both failed, the confirmation test with the same `the server is draining` refusal |
| fix, three runs | none | both passed each |
| `drain_end.rs` on TCP and UDP, the idle end rewritten to `std::future::pending::<()>()` | none | both failed: "the server's stop took 30.00…s with no call held, against a drain window of 30s" |
| fix, the same test | none | both passed in 0.01 s or less |
| TCP `drain_window` six times, `close`'s clear of `State::connections` rewritten out | none | 4 failed: "call 0 of 1 sent during the drain was not refused `Unavailable`, got: Err(Timeout)" |
| fix, six times | none | 6 passed |
| `drain_window` on NATS, its drain made to neither unsubscribe nor deliver, declaring `unconfirmed_drain` | that edit | passed |
| the same, declaring `unconfirmed_drain(false)` | that edit | failed: "call 0 of 5 sent during the drain was not refused `Unavailable`, got: Err(Timeout)" |

The 15 ms probe is batch 15's: a `tokio::time::sleep` before each poll of the server's event
loop, inserted by a script and removed. The last two rows show the scenario's check against a
drain known to lose every call; the edit was removed afterwards.

Four earlier MQTT runs are not evidence and are not counted: the first script passed the two test
names as one zsh word, so libtest's filter matched nothing, reported `running 0 tests` and exited
0; a probe inserted by text that matched both event loops was refused by its own assertion and
never ran. Their output is kept apart in the scratchpad (`runs/invalid/`).

## Found, filed

- **F353** (filed and fixed, decision 6): the TCP and UDP links ended their inbound stream only
  when every connection closed, or at `close`.
- **F356** (filed and fixed, decision 6): TCP's `close` left an aborted connection's socket open.
- **F357** (filed open): a Kafka server's `bind` fails when the broker's group coordinator is not
  yet ready; seen once in the verification, unrelated to the batch.
- **F354:** the Redis link drops a request it reads after its drain ended the inbound stream, the
  watcher's take and `hold` running under no common lock. Unprobed.
- **F355:** MQTT 5 lets a broker deliver messages buffered for the client after the UNSUBACK; one
  read after `close` queued the DISCONNECT is lost. Unprobed.
- UDP's call lost at close is declared rather than filed: it is the capability's case.

## Left for the transports DESIGN fold

- §5.3's `Capabilities` block (line 932): `unconfirmed_drain` after `durable_replies`; the
  paragraph after it (line 942), "`new` sets `holds_unserved` and `durable_replies` to `false`",
  now covers `unconfirmed_drain` too, with NATS as the one link declaring it and what a caller
  sees.
- §5.3's shutdown paragraph (line 962): "async-nats's public API offers no way to hold the
  subscription for the `PONG`, and that window is a stated limit of the NATS link's drain" is now
  the declared `unconfirmed_drain`; "MQTT unsubscribes each `$share` filter" now waits for each
  UNSUBACK under the drain deadline, logging the unconfirmed filters when the deadline drops it;
  "the inbound stream ends once the draining server holds no call" holds on MQTT only after the
  UNSUBACKs; and `Server::drain`'s wait for the inbound stream and the refusals, with `close`'s
  `warn`, is described nowhere.
- §5.2's conformance paragraph (line 878): the `drain_window` scenario, and `unconfirmed_drain`
  among the places the caller's own `Timeout` passes.
- §5.3's link table, UDP's row (line 955 onward): a call sent as the server closes can be lost,
  declared `unconfirmed_drain`.
- §5.3's shutdown paragraph (line 962): "TCP closes its listener and sends `goaway` on every
  connection ... and finishes the calls in flight" and "UDP sends nothing and keeps receiving, so a
  `cancel` still arrives" now go on to end the inbound stream once no call is held and answer later
  requests `unavailable` from the link; TCP's `close` releases the writers of connections whose
  tasks it aborted.
- §10's table (line 1166): the RPC `drain` cell, "MQTT unsubscribe", now waits for the UNSUBACKs;
  the drain-window cell gains the inbound stream's end and the refusals.
- §12's row "A call arriving once `Execution::open` is refused" (line 1266): on a link declaring
  `unconfirmed_drain` such a call can see its own `Timeout`.
- Decision 34 (line 1328): `unconfirmed_drain` is declared and asserted beside `holds_unserved` and
  `durable_replies`.
- §2.6 (line 313) and §3's `HEAD` bullet (line 436) are consistent with decision 5 and need no
  change; `on_stream_end`'s HTTP-only sentence already reads there as "A stream discarded before the
  service sees it reports nothing".

## Needs sign-off

### S1. `close` drops what the inbound stream has ready, and counts it

Decision 1: the count of unread deliveries is taken by draining the stream without waiting at
`close` and dropping each delivery. The alternative is to count nothing the server has not read,
leaving them in the stream to drop with it, which loses the count the response asked for. A
dropped delivery is not refused: refusing would need a send that `close` does not wait for.

### S2. TCP and UDP were changed, against the response's "report, do not patch"

Decision 6: the response asked for a link breaking the stream's promise to be reported, not
patched; the coordinator then ruled a drain waiting out its deadline with nothing pending a
defect, and TCP and UDP now end their stream once idle, as the broker links do. The alternative
the response's rule implied, a TCP or UDP server taking the whole drain window to stop whenever a
client is connected, is what the change removes.

### S3. MQTT's unconfirmed-unsubscribe log fires from a guard's drop

Decision 2: the response left the place open. The core's deadline path has no logger and no
filters; the guard's drop sees both. It also fires if the future is dropped for another reason,
the shutdown cap among them, and its message names the deadline.

### S4. The capability's name, `unconfirmed_drain`, and the scenario that asserts it

Decision 3: the name, and `drain_window` as a new scenario rather than a change to `drain`, whose
call `settle / 2` into the drain keeps requiring `Unavailable` from NATS. The new scenario's calls
during the drain on Redis can meet F354's window; none did in the runs below.

### S5. Batch 14's `on_stream_end` sentence is in `ulo-http`, not on `Execution::on_stream_end`

Decision 5: the sign-off placed it on `on_stream_end`'s doc and in `ulo-http`. The core's doc
serves RPC too, where a discarded `Reply::Many` reports `CutOff`; written there, the sentence would
be false for RPC. It is on `HttpBody::stream` and in `AppService`'s step 8.

### S6. UDP declares `unconfirmed_drain`

Decision 3: the response named the capability for NATS; the scenario asserting it found UDP losing
a call that raced its close. The alternatives are to accept the declaration, to have the scenario
stop sending before the drain could reach its deadline, which would hide the loss, or to treat it
as a UDP defect, though no change to the link removes a datagram's fate in a socket that closes.

## Verification

Full, unfiltered output of every run is in the session scratchpad: `runs/` for the probes and the
before/after runs, `verify/` for the final pass below, and `verify-before-udp-capability/` and
`verify-before-tcp-udp-idle-end/` for the two passes stopped when they found UDP's lost call and
the TCP drain waiting out its window.

Conformance, three consecutive runs per suite on the final tree, one suite at a time, test time as
libtest reports it; the brokers with `--features integration --test conformance --locked`:

| Suite | Run 1 | Run 2 | Run 3 | Result |
| --- | --- | --- | --- | --- |
| `ulo-rpc-tcp` conformance | 1.31 s | 1.31 s | 1.31 s | 25 passed |
| `ulo-rpc-tcp` conformance_cbor | 1.31 s | 1.30 s | 1.31 s | 25 passed |
| `ulo-rpc-udp` conformance | 1.31 s | 1.31 s | 1.31 s | 24 passed, 1 ignored |
| `ulo-rpc-nats` | 13.47 s | 8.66 s | 6.46 s | 25 passed |
| `ulo-rpc-redis` | 7.87 s | 7.81 s | 8.27 s | 25 passed |
| `ulo-rpc-mqtt` | 10.64 s | 8.93 s | 10.36 s | 29 passed: the suite's 25 and four link tests |
| `ulo-rpc-kafka` | 31.45 s | 30.60 s | 42.54 s | 25 passed |
| `ulo-rpc-rabbitmq`, `ULO_CONFORMANCE_PARALLEL=6` | 129.28 s | 142.82 s | 131.62 s | 25 passed |

Kafka's first run of the pass failed two scenarios at startup, "transport `Rpc` failed to bind:
Meta data fetch error: NotCoordinator (Broker: Not coordinator)", filed as F357. The table shows
runs 2, 3 and 4 of six; runs 2 to 6 passed one after another, runs 5 and 6 in 40.98 s and 31.50 s.
Its two failure files are kept in `target/conformance-failures/` and in the scratchpad.

No scenario waits out a drain window. Each scenario run alone on the final tree: TCP 0.03–1.34 s,
UDP 0.02–1.33 s, MQTT 0.44–1.98 s, NATS 0.42–2.53 s, Redis 0.42–1.85 s, apart from
`recovery_after_disrupt` at 4.01 s on MQTT and 4.34 s on Redis, its three-second outage plus the
environment's settle (`runs/<link>-per-scenario*.txt`). The suites' longer totals beside batch 14's
(NATS 6.5–13.5 s against 5.0–6.8 s, MQTT 8.9–10.6 s against 5.9–7.0 s) come from running the
scenarios in parallel on a host at load averages of 5 to 7; no scenario alone takes as long as the
conformance app's five-second drain window.

- `cargo check --workspace --all-targets --all-features` with
  `CFLAGS=-I/opt/homebrew/opt/openssl/include LDFLAGS=-L/opt/homebrew/opt/openssl/lib`, and `cargo
  +1.88 check --workspace --all-targets --exclude ulo-http-salvo --exclude
  ulo-graphql-async-graphql`: each prints the 17 known warnings in `crates/ulo/src` and no other.
- `cargo test --workspace --no-fail-fast` with the OpenSSL flags: 438 passed, 0 failed, 64
  ignored: batch 16's 430, the three tests of `crates/ulo-rpc/tests/drain_window.rs`, the two
  `drain_end.rs` tests, and `drain_window` in the TCP, TCP CBOR and UDP suites. The MQTT link tests
  are behind `integration` and run in the MQTT suite above.
- `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --lib -p <crate>` for `ulo`, `ulo-rpc`,
  `ulo-rpc-nats`, `ulo-rpc-mqtt`, `ulo-rpc-tcp`, `ulo-rpc-udp`, `ulo-rpc-conformance` and
  `ulo-http`: each passes; the only warnings printed are the core's 17 compiler warnings.
- `cargo +1.98.1 clippy -p <crate> --all-targets --no-deps --locked` for the same eight, and for
  `ulo-rpc-mqtt` and `ulo-rpc-nats` again with `--features integration`: no warning on a line this
  batch wrote. The warnings printed sit on lines it did not touch: `type_complexity` at
  `crates/ulo/src/transport/server.rs:229` and `crates/ulo-rpc/src/link.rs:136` and `:179`,
  `crates/ulo-rpc/src/client.rs:71`, `result_large_err` in `crates/ulo-rpc/src/extract.rs`, batch
  15's `clone_on_copy` at `crates/ulo-rpc-mqtt/src/link.rs:643`, `ulo-rpc-conformance`'s two at
  `cases/app.rs:468` and `lib.rs:281`, and `ulo-http`'s at `service.rs:504` and `:708` among others
  outside `body.rs:36-40` and `service.rs:65-77`.

### Containers

Every suite started its own containers and removed them; the counts before and after each run are
in `verify/summary.txt`. The running set was the user's four containers and ten leftover
`eclipse-mosquitto:2.0.18` containers until the coordinator removed the ten at the user's request
during the second pass, which the count records as 14 to 4 across `tcp-conformance_cbor-1`, a run
that starts no container. Every count since is 4 before and after. The user's containers were not
touched, and the Docker engine answered throughout.
