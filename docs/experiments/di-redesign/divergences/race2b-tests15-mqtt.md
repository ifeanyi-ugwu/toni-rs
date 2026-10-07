# Divergences: race 2b, tests batch 15, MQTT's drain answering a refused call `Timeout` (F347)

CI run 37684524627 (`5231c903`) failed the RPC conformance `drain` scenario on the MQTT link at
`crates/ulo-rpc-conformance/src/cases/drain.rs:50`: the call sent during the drain got the caller's
own `Timeout`, where a link declaring `miss_signal` must answer `Unavailable`. The CI failure did
not reproduce in 420 loaded iterations. Three orderings in the link produce that exact failure, and
each was shown by a probe that widens its window on the unchanged code: the client registering a
request's packet id after the broker's PUBACK 0x10 can already have been read; the server's
`close` aborting its event loop before it writes the refusal of a call that arrived during the
drain; and the server dropping a request that arrives once its inbound stream has ended. The loops
also reproduced a fourth defect twice, on the same `close`: the held call's own reply lost, failing
`drain.rs:57` (F348). All four are fixed in the link.

Files changed: `crates/ulo-rpc-mqtt/src/link.rs`, `docs/experiments/di-redesign/transports/DESIGN.md`.

## Reproducing it

The harness is a temporary test file in `crates/ulo-rpc-mqtt/tests/`, with `tracing-subscriber` as
a temporary dev-dependency, both removed afterwards. It runs
`ulo_rpc_conformance::cases::drain::drain::<Mosquitto>()`, `Mosquitto` copied from
`crates/ulo-rpc-mqtt/tests/conformance.rs` (`eclipse-mosquitto:2.0.18`, the client through the
relay), once per iteration under `catch_unwind` on a multi-thread runtime with a set number of
workers. A subscriber writes into a buffer at
`ulo_rpc=trace,ulo_rpc_mqtt=trace,rumqttc=trace,ulo=debug`, cleared per iteration and written to a
file when the iteration fails. rumqttc logs through `log`, bridged by `tracing-log`, and writes a
debug line for each outgoing PUBLISH, SUBSCRIBE and UNSUBSCRIBE. `ulo_rpc` and `ulo_rpc_mqtt` write
nothing on this path, so the link carried temporary trace lines, also removed: every event both
event loops poll, the drain's start, the inbound stream's end, and `close`.

Before any fix, on a 10-core machine, Docker through OrbStack, swap at 15.6 of 16 GB, debug
profile:

| Broker | Conditions | Iterations | Failed |
| --- | --- | --- | --- |
| a container per iteration | three processes, two workers each, eight `yes` hogs | 120 | 1, `drain.rs:57` |
| one container per process | four processes, two workers each, sixteen `yes` hogs | 300 | 1, `drain.rs:57` |

With one container per process, each iteration still builds its own apps, links and relay against the
shared broker. Load averages ran 14–27. No iteration failed at `drain.rs:50`.

## The orderings

### The client registers the packet id after the PUBACK

`ClientSide::publish` queued the PUBLISH, waited on a oneshot for the packet id that the client's
event loop sends on the `Outgoing::Publish` event, and then inserted `pkid -> call` into `pkids`.
rumqttc writes and flushes the packet before it returns that event (`rumqttc-0.25.1`,
`src/v5/eventloop.rs`, `select`, lines 226–231). From the flush on, the PUBACK can be read by the
event loop, and `acknowledged` finding no entry for the packet id discards a 0x10 (`return` on a
missing `pkids` entry). The caller then waits out its own timeout. The publishing task inserts
only once it is scheduled again, so the window is one broker round trip against that task's next
poll.

Probe: a busy-wait in `publish` between the oneshot and the insert, standing for the publishing
task preempted there.

| Delay | Iterations | Failed at `drain.rs:50` |
| --- | --- | --- |
| 200 µs | 5 | 0 |
| 300 µs | 5 | 0 |
| 500 µs | 5 | 2 |
| 1 ms | 5 | 5 |
| 2 ms | 5 | 4 |
| 5 ms (a `tokio::time::sleep`) | 5 | 5 |

Each failure carries CI's message: "a call arriving during the drain is refused `Unavailable`, got:
Err(RpcError { kind: Timeout, message: "the call timed out", .. })". The trace of the first 1 ms
failure, its uptime timestamps:

```text
0.720774875s f347: link drain: phase Draining, unsubscribing
0.720886917s rumqttc::v5::state: Unsubscribe. Topics = ["$share/ulo_rpc_conformance::cases::app::ServerRoot/conformance.add"], Pkid = 6
0.997436750s rumqttc::v5::state: Publish. Topic = conformance.add, Pkid = 5, Payload Size = 13
0.998315834s f347: side="client" event=Incoming(PubAck(PubAck { pkid: 5, reason: NoMatchingSubscribers, properties: None }))
1.498223584s f347: link drain: no call held, deliveries taken
1.498386000s f347: link close
1.999696292s PANIC: panicked at crates/ulo-rpc-conformance/src/cases/drain.rs:50:18
```

The broker answered 0.88 ms after the PUBLISH, and the client dropped the answer. The round trip
here crosses the relay and OrbStack's VM; on a Linux runner with a native engine it is shorter, so
a shorter preemption suffices.

### The server's `close` aborts the refusal

The drain's UNSUBSCRIBEs go through rumqttc's request channel behind whatever replies are already
queued, and the request branch is closed while the broker's Receive Maximum (20 for Mosquitto) of
publishes awaits acknowledgment. Until the broker has processed the UNSUBSCRIBE it routes a request
to this instance. The server answers such a request `err` of kind `unavailable`, its execution
refused once the drain began (`crates/ulo-rpc/src/dispatch.rs`, `start`), and the refusal is again
a queued request. `Mqtt::close` queued a DISCONNECT and aborted the event loop at once, so whatever
the loop had not yet written was lost: the refusal, and the held call's reply.

Probe: a 15 ms `tokio::time::sleep` before each poll of the server's event loop, standing for a
loop behind its queue.

```text
1.110816917s f347: link drain: phase Draining, unsubscribing
1.385959375s rumqttc::v5::state: Publish. Topic = conformance.add, Pkid = 5, Payload Size = 13
1.386461334s f347: side="client" event=Incoming(PubAck(PubAck { pkid: 5, reason: Success, properties: None }))
1.414482250s rumqttc::v5::state: Unsubscribe. Topics = ["$share/ulo_rpc_conformance::cases::app::ServerRoot/conformance.add"], Pkid = 9
1.519505584s f347: side="server" event=Incoming(Publish(Publish { .. topic: b"conformance.add", payload: b"{\"a\":1,\"b\":1}", .. }))
1.519570542s f347: delivered to the server sent=true
1.876639334s f347: link drain: no call held, deliveries taken
1.876769584s f347: link close
2.389151625s PANIC: panicked at crates/ulo-rpc-conformance/src/cases/drain.rs:50:18
```

The UNSUBSCRIBE for `conformance.add` left 28 ms after the refused call's PUBLISH, which the
broker acknowledged `Success` and routed to the server. The refusal and the held call's reply never
appear as outgoing publishes. 10 of 10 iterations failed this way; with a 3 ms sleep, 0 of 10.

The two unprobed failures at `drain.rs:57` are the same loss without the lag: 70 µs between the
inbound stream ending and `close`, and no outgoing publish of the held call's
`{"t":"res","id":3,"d":1300}`. From the second (one container per process, iteration 62):

```text
84.740750041s rumqttc::v5::state: Publish. Topic = conformance.add, Pkid = 5, Payload Size = 13
84.741021833s f347: side="client" event=Incoming(PubAck(PubAck { pkid: 5, reason: NoMatchingSubscribers, properties: None }))
85.247224166s f347: link drain: no call held, deliveries taken
85.247295750s f347: link close
113.946655333s PANIC: panicked at crates/ulo-rpc-conformance/src/cases/drain.rs:57:21:
               the in-flight call finishes during the drain: RpcError { kind: Timeout, .. }
```

The reply path releases the call before it queues the reply, so the drain's watcher ends the
inbound stream, the core sees no live execution and calls `close`, and the abort can come before
the event loop takes the reply from its channel. The first failure ran through a six-second stall
of the whole process, read as swapping; the second had none.

### The server drops a request after its inbound stream ends

The drain's watcher ends the inbound stream once the server holds no call, and `ServerSide::deliver`
then discarded what it was handed. A request the broker routed before it processed the
UNSUBSCRIBE, arriving after that point, got no answer at all.

Probe, on a tree with the first two fixes: the watcher ending the inbound stream at once instead of
waiting for no held call, with the 15 ms lag. 10 of 10 iterations failed at `drain.rs:50`.

### Which one CI hit

CI's log carries only the assertion. The first ordering needs one task preempted for about a
broker round trip at one point in a call; the suite runs its scenarios in parallel, each with its
own broker container and relay, on a 2–4 core runner. The second needs the server's event loop
behind its queue by 250 ms when the refused call is published (the scenario's `settle / 2`) and
still behind when the held call finishes about 550 ms later. The first is the likelier on that
evidence; neither is shown to be the CI failure.

## Which side is wrong

Transports DESIGN §5.2 and the shutdown table in §10: a call arriving during the drain is refused
`Unavailable` on a link with a miss signal. The core and the dispatcher hold to it, answering a
call that reaches a draining server with its refusal. The MQTT link lost the answer at three
points: the miss signal on the client, the refusal and the drained call's reply in `close`, and
the request itself after the inbound stream ended.

## The fix

- **The client:** the packet id is mapped to its call by the event loop, on the publish's own
  `Outgoing::Publish` event. `ClientSide::outgoing` holds a `Waiting { call, assigned }`; the event
  loop writes `pkids` before sending the id on, and rumqttc returns events in order, so the entry
  exists before the PUBACK's event. A collision's `AwaitAck` defers the `Outgoing::Publish` until
  the earlier publish's acknowledgment, which rumqttc returns as an event ahead of it, so the
  earlier publish's PUBACK cannot claim the new entry.
- **`close`:** the server side's `close` queues the DISCONNECT and awaits the event loop, which
  returns once it reports `Outgoing::Disconnect` with the phase `Closed`. rumqttc writes requests
  in the order they were queued, so every reply queued before the DISCONNECT has been written
  ahead of it. A connection that fails after `close` ends the loop instead of reconnecting. The
  wait runs under the core's `close` bound; a guard aborts the loop when that bound drops the
  future. A DISCONNECT rumqttc writes on a protocol error while serving does not end the loop.
- **The late request:** `deliver` reports whether the inbound stream took the delivery. A `req` it
  did not take is released from the call table and answered `err` of kind `unavailable`, "the
  server is draining", the dispatcher's own refusal, published with `try_publish` from the event
  loop. An `open` is checked under the lock on the inbound sender and refused the same way before
  `opened` is sent, so the caller is never told `opened` for a call the server will not receive.

## Verification

Each probe against the fixed code, the same delay in the same place:

| Probe | Before | After |
| --- | --- | --- |
| publishing task delayed 1 ms after the packet id | 5 of 5 failed | 0 of 10 |
| publishing task delayed 5 ms | 5 of 5 failed | 0 of 10 |
| server event loop 15 ms behind | 10 of 10 failed | 0 of 10 |
| inbound stream ended at once, loop 15 ms behind | 10 of 10 failed | 0 of 10 |

The first three "after" rows ran with the first two fixes, the last with all three. Loops after the
fix, one container per process, debug profile:

| Fixes | Conditions | Iterations | Failed |
| --- | --- | --- | --- |
| client and `close` | four processes, two workers each, sixteen `yes` hogs | 400 | 0 |
| all three | four processes, two workers each, sixteen `yes` hogs | 400 | 0 |
| all three | three processes, four workers each, eight `yes` hogs | 210 | 0 |
| all three, as committed | three processes, two workers each, eight `yes` hogs | 150 | 0 |

1,160 iterations after the fix, none failing, against two failures in 420 before it and every
probe's failures. The rows before the last ended the server loop on any `Outgoing::Disconnect`;
the committed code ends it only with the phase `Closed`, which in these runs is the only DISCONNECT
written. Load averages ran 20–44. The 400 iterations with the first two fixes are not evidence for
the third ordering, which the scenario reaches only under the probe.

In the repository, without the harness or the trace lines:

- `cargo test -p ulo-rpc-mqtt --features integration --test conformance`, three runs in a row: 24
  passed each time, in 5.92 s, 5.12 s and 5.27 s. Full output of each is kept in the session
  scratchpad. No other broker's link was touched.
- `cargo check --workspace --all-targets --all-features` on stable, with
  `CFLAGS=-I/opt/homebrew/opt/openssl/include LDFLAGS=-L/opt/homebrew/opt/openssl/lib`: the same 17
  warnings, all in `crates/ulo/src`, the set listed in `batch2a-cfgattr.md`. Without the flags
  `rdkafka-sys` fails its build on `openssl/ssl.h`, as in batches 13 and 14.
- `cargo clippy -p ulo-rpc-mqtt --all-targets --features integration`: one warning from the crate,
  `clone_on_copy` on the SUBACK reason code in `server_loop`, a line this batch did not change.
- `cargo test --workspace --no-fail-fast`, with the OpenSSL flags: 430 passed, 0 failed, 64
  ignored, batch 14's count.

## Found by reading, not fixed

- **The client side's `close` aborts its event loop the same way** (F349). `Mqtt::close` queues a
  DISCONNECT on the client connection and aborts the loop at once, so an `emit` or a `cancel`
  queued just before it can be lost. Unprobed.
- **The drain's watcher can end the inbound stream with a call held** (F350). It waits for the
  call table to empty and then takes the inbound sender. A request held in between is delivered,
  and its `in`, `in_end` and `cancel` frames afterwards are dropped, since they travel on the
  inbound stream: a streamed request opened in that window stalls until its deadline or the drain's
  end. Unprobed.
- **Each pattern costs one UNSUBSCRIBE** at the drain, each on its own packet id from the counter
  the publishes share, which Mosquitto's Receive Maximum caps at 20; the drain of the conformance
  app's 21 patterns wraps it. `AsyncClient::unsubscribe` takes one filter, and an MQTT UNSUBSCRIBE
  can carry many, which rumqttc's v5 client does not expose. Not a defect: packet ids for
  UNSUBSCRIBE and for a QoS 1 PUBLISH collide only while both are unacknowledged, and the traces
  show no UNSUBACK or PUBACK out of place.

## The leftover containers

The harness's runs with one container per process kept the container in a static, which is never
dropped, so the first versions of that mode left their containers running when the process exited;
three more stayed when a runner was stopped mid-loop. Ten `eclipse-mosquitto:2.0.18` containers
from the harness remain on the engine: `intelligent_solomon`, `stoic_cray`, `boring_bouman`,
`sweet_galileo`, `priceless_brahmagupta`, `mystifying_chandrasekhar`, `adoring_curran`,
`dazzling_murdock`, `reverent_joliot`, `adoring_maxwell`. Removing them was refused by the session's
permission check; `docker rm -f` on those names removes them. The later harness dropped its
container at the end of each loop.
