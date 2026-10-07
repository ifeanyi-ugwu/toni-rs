# Divergences: race 2b, tests batch 5, the RPC conformance suite on the five broker links

This batch wires `ulo-rpc-conformance` into the NATS, Redis, RabbitMQ, MQTT and Kafka links, each
suite against a real broker in a container, and runs them. The links had never met a broker
(`race2b-B.md`, "Not verified"). Four of five failed the recovery scenario the same way, RabbitMQ
failed the drain on the risk `race2b-tests3.md` S4 left open, and Kafka lost records across a
rebalance. The fixes are in all five links, in the suite's environment, and one new capability.
Every suite passes three runs in a row on the final tree.

Files added: `crates/ulo-rpc-{nats,redis,rabbitmq,mqtt,kafka}/tests/conformance.rs`,
`crates/ulo-rpc-conformance/src/relay.rs`. Files changed:
`crates/ulo-rpc-{nats,redis,rabbitmq,mqtt,kafka}/Cargo.toml` (an `integration` feature,
dev-dependencies `testcontainers`, `tokio/rt-multi-thread`, `ulo-rpc-conformance`),
`crates/ulo-rpc-{nats,redis,rabbitmq,mqtt,kafka}/src/{link.rs, lib.rs}`, `crates/ulo-rpc/src/link.rs`,
`crates/ulo-rpc-conformance/{Cargo.toml, src/lib.rs, src/cases/drain.rs}`,
`crates/ulo-rpc-tcp/tests/conformance.rs`, `Cargo.lock`.

## The signatures

```rust
// ulo_rpc
pub struct Capabilities { /* .. */ pub holds_unserved: bool }       // new field; `new` sets `false`
impl Capabilities { pub const fn holds_unserved(self, holds_unserved: bool) -> Self; } // new

// ulo_rpc_conformance::relay (new module)
pub struct Relay;
impl Relay {
    pub async fn start(upstream: SocketAddr) -> Relay;
    pub async fn bind() -> tokio::net::TcpListener;
    pub fn listen(listener: tokio::net::TcpListener, upstream: SocketAddr) -> Relay;
    pub fn addr(&self) -> SocketAddr;
    pub async fn cut(&self);
}
pub async fn reachable(addr: SocketAddr, within: Duration);
```

`Capabilities` is `#[non_exhaustive]` and built through `new` and setters, so no caller breaks.
RabbitMQ and Kafka declare `holds_unserved: true`; every other link keeps `false`. The relay is
TCP's former private one, moved so the five broker suites and TCP share it.

## The environments

| Link | Image | Isolation | `disrupt` | `budget` |
| --- | --- | --- | --- | --- |
| NATS | `nats:2.10.14` | a container per scenario | cuts the client's relay | default |
| Redis | `redis:7-alpine` | a container per scenario; Pub/Sub channels span every database | cuts the client's relay | default |
| RabbitMQ | `rabbitmq:4.1-alpine` | a container per scenario | cuts the client's relay | default |
| MQTT | `eclipse-mosquitto:2.0.18`, run with `/mosquitto-no-auth.conf` | a container per scenario | cuts the client's relay | default |
| Kafka | `apache/kafka-native:3.8.0`, single-node KRaft | a container per scenario | cuts the client's relay | boot 30 s, settle 3 s, recovery 30 s |

Every server link connects to the broker's forwarded port and every client link to a relay in
front of it, so `disrupt` severs the client's connection and nothing else, as `Broker::disrupt`
documents. A Kafka client connects wherever the broker's metadata advertises, not where it
bootstrapped, so the Kafka broker has a listener for the server and one for the client, each
advertising a relay bound before the container starts. `KAFKA_GROUP_INITIAL_REBALANCE_DELAY_MS=0`
spares each group's first join three seconds. Scenarios run in parallel with no cap on
containers; the RabbitMQ suite, twenty brokers, finishes in 12 to 14 s.

What each suite declares not applicable:

| Link | Declared not applicable |
| --- | --- |
| NATS, Redis, RabbitMQ, MQTT | none |
| Kafka | `recovery_after_disrupt`: the reply topic is durable and librdkafka reconnects by itself, so a severed connection loses no reply and the waiting call is answered |

## Fixes

### 1. A forwarded port refused connections after the broker reported ready

- **What forced it:** the first NATS run failed `binary_payload` and `bidi_stream` with "the
  conformance server did not start: transport `Rpc` failed to bind: IO error: Connection refused
  (os error 61)". The container had logged "Server is ready"; Docker Desktop's port forward was
  not yet accepting.
- **Written:** `relay::reachable(addr, within)` connects until one succeeds, and every broker's
  `start` waits on it, 10 s at most, before handing out the address.

### 2. Four links carried a call across a lost connection instead of failing it

- **What forced it:** `recovery_after_disrupt` failed with "a call waiting on a severed connection
  fails `Unavailable`, got: Ok(3000)" on NATS, Redis and MQTT. On RabbitMQ the call never finished:
  "the call waiting on the severed connection did not finish within 13s".
- **Cause:** each client side outlived its connection, and `RpcClient` fails the calls waiting on
  a reply lane only when the lane ends (DESIGN §5.4: "A lost reply lane fails every call waiting on
  it `Unavailable`"). async-nats reconnected and resubscribed the inbox; the Redis client lane
  resubscribed its reply channel after 500 ms; the MQTT client loop reconnected and resubscribed
  on the next CONNACK. Each reconnect finished before the 3 s reply, so the reply arrived. A reply
  published during the gap is lost on all three, and a longer outage would have ended the call at
  its own `Timeout`. lapin's auto-recovery re-consumed `amq.rabbitmq.reply-to` on a new channel,
  and the replies addressed to the old channel were gone.
- **Written:**
  - NATS: `connect` registers an `event_callback`, and `Event::Disconnected` ends `route_replies`.
  - Redis: `client_lane` returns when the Pub/Sub stream ends, without resubscribing.
  - MQTT: `client_loop` returns on a connection error after the first CONNACK.
  - RabbitMQ: the client connection is opened without `enable_auto_recover`, and `route_replies`
    ends on the consumer's `Err`.
  Ending the lane drops the frame sender, `RpcClient` fails the waiting calls `Unavailable`, and
  the next call connects through a fresh `Link::connect`. The server sides keep their own
  reconnects. No signature changed. Each crate's doc states the behaviour.

### 3. Kafka skipped records produced while its group had no committed offset

- **What forced it:** `two_instances` failed with "9 of 10 deliveries arrived under Competing"
  in two of three suite runs and one of three runs alone. Separately, the drain's held event
  (decision 1) never reached the next instance.
- **Cause, read from librdkafka's `cgrp` log:** the link consumes with `auto.offset.reset=latest`
  and commits only offsets a handler settled. A partition the group has never settled has no
  committed offset, so every assignment of it starts at its end. The second instance's join
  triggers an eager rebalance, and an event produced while the partition was unassigned is behind
  the end the next owner starts at. In the drain, `conformance.event` had no committed offset, and
  the next instance, assigned all 15 partitions 10 ms after joining, started past the event. This
  contradicts the link's own doc, "Unsettled, a restarted group reads the record again".
- **Written:** `listen` calls `anchor` after creating the topics: for every handler partition
  where the group has no committed offset, it commits the partition's high watermark from outside
  the group. A partition then starts where the group first bound. Kafka accepts that commit only
  while the group is empty, so an instance joining a running group leaves it to the instances
  already there and logs the refusal at `debug`.
- **Checked:** `two_instances` passed eleven of eleven afterwards, three alone and eight in
  parallel, and the held event reaches the next instance.

## Decisions

### 1. RabbitMQ's drain answer is declared: `Capabilities::holds_unserved`

- **What the run showed:** `drain` failed on RabbitMQ with "a call arriving during the drain is
  refused `Unavailable`, got: Err(RpcError { kind: Timeout, .. })", as `race2b-tests3.md` S4
  predicted. The drain cancels the consumers, the pattern's queue keeps the request for the next
  instance, and the caller times out.
- **Against the design:** DESIGN §5.2 states this outcome for RabbitMQ: "a request for a pattern
  served once waits in the queue for the next instance and the caller sees its own `Timeout`
  though the link declares `miss_signal`". The suite's rule from `race2b-tests3.md` decision 12
  accepts `Timeout` only where `miss_signal` is false. The suite was wrong against the design, but
  had no field to read the design's statement from.
- **Written:** `Capabilities::holds_unserved`, "the broker keeps a request for a pattern a server
  has subscribed while no instance consumes it, and hands it to the next instance". The drain
  accepts the client's `Timeout` where `!miss_signal || holds_unserved`. Where `holds_unserved`
  is declared, the scenario also emits an event during the drain, starts a second server once the
  first has closed, and requires the event there, so the declared `Timeout` does not pass on
  silence. RabbitMQ and Kafka (after fix 3) declare it.
- **Checked against a violation:** NATS with `holds_unserved(true)` written in temporarily failed
  with "the event emitted during the drain did not reach the next instance, though the link
  declares `holds_unserved`"; NATS loses an event nothing consumes. Kafka declaring it before
  fix 3 failed the same way, which is how fix 3 was found.
- **Alternatives:** declaring `drain` not applicable on RabbitMQ, which drops its in-flight and
  stream assertions; or having RabbitMQ's drain keep consuming and refuse new calls, which
  contradicts §5.3's `basic.cancel` and refuses requests a running instance could take.

### 2. Kafka's recovery scenario is declared not applicable

- **What the run showed:** `Ok(3000)`, with `disrupt` cutting every client connection through the
  relay. librdkafka reconnects through the relay, the reply topic kept the reply, and the waiting
  call was answered. Run with `--ignored`, it fails the same way.
- **Why not a link fix:** the other four links lose what is published during the gap, so ending
  their reply lane reports a loss that happened. Kafka's reply topic loses nothing, and ending its
  lane would turn an answered call into a failure.
- **Cost:** no scenario covers a Kafka client reconnecting after a severed connection. Closing that
  needs the recovery scenario to accept an answered call on a link that declares a durable reply
  lane, which is another capability.

### 3. One relay for the six socket suites

- **Written:** `Relay` moved from the TCP suite into `ulo_rpc_conformance::relay`. The TCP suite
  uses it unchanged in behaviour; `Relay::bind` and `Relay::listen` exist for Kafka, whose relays
  must have addresses before the broker starts.

## Needs sign-off

### S1. `Capabilities::holds_unserved`

A public field and setter on `ulo_rpc::Capabilities` (decision 1), true on RabbitMQ and Kafka. The
alternatives are in decision 1.

### S2. Kafka's group offsets are anchored at `bind`

Fix 3 changes where a Kafka group starts on a partition it has never committed: the end at the
group's first bind, not the end at each assignment. A deployment that adds a handler while the
group runs still starts that topic at its end on first assignment, since the commit is refused
for a non-empty group.

### S3. Kafka's recovery scenario declared not applicable

Decision 2. The alternative is the capability its last paragraph names.

### S4. DESIGN text these changes falsify, left for the fold

- §5.3's `Capabilities` block lacks `holds_unserved`.
- §5.2's conformance paragraph says the drain refuses a new call `unavailable`; RabbitMQ and Kafka
  answer `Timeout`, declared, with the held event asserted.
- §5.2's RabbitMQ sentence ends "though the link declares `miss_signal`"; it also declares
  `holds_unserved`.
- §5.3's limits table, Kafka row, lacks the anchored offsets of fix 3.
- §5.3's "After bind, NATS and lapin recover by themselves" holds for the server sides. A client
  side now ends its reply lane on a lost connection, and the RabbitMQ client no longer uses lapin's
  recovery.

## Findings, not fixed

### F1. `RpcClient` never closes its link

`RpcClientModule` registers the client as a singleton with no shutdown hook, and nothing calls
`Link::close` on the client side. The Kafka client's reply consumer therefore lives until the
runtime shuts down. In the suite that is after the scenario removed the broker, and rdkafka's
consumer drop then waits on a group leave that cannot be answered: a stack sample showed a worker
inside `BaseConsumer::drop`, called from the dropped `route_replies` task, for about 45 s, Kafka's
default session timeout. That is about 45 s of each Kafka scenario's 50 to 55 s; the scenarios run
in parallel and the suite takes about 100 s. Closing the link on the client app's shutdown is the
fix; it adds a hook to `RpcClientModule`.

### F2. A dropped `StreamConsumer` can block in `rd_kafka_destroy`

A probe (removed) subscribed a `StreamConsumer`, produced until it received a record, and dropped
it. The drop left the group (LeaveGroup answered in 15 ms), then blocked in `rd_kafka_destroy`
joining librdkafka's main thread, past a 40 s watchdog, in every run. A `BaseConsumer`, an
unpolled `StreamConsumer`, and one that received a single record dropped in under 110 ms. Storing
offsets, committing, pausing, unsubscribing and draining unread records did not change the
outcome, and the trigger is not isolated. The Kafka link drops its consumers on a tokio worker at
`close`; in the conformance runs the server consumers did not block (a sample during the drain
found no thread in `rd_kafka_destroy`).

### F3. An unreproduced NATS drain failure

The first NATS run of the earlier three-run set failed `drain` at "the in-flight call finishes
during the drain": the held `HOLD` call received no reply and ended at its own 30 s timeout. The
panic message was not kept beyond its location. 47 further runs, three and eight suite processes
at a time, did not reproduce it, and the three runs on the final tree passed. Cause not found.

## The race2b-B "Not verified" items the runs now cover

Each was exercised by a passing scenario: direct reply-to working on a confirm-mode channel, and
a `mandatory` publish's `basic.return` reaching the confirmation (RabbitMQ `unhandled_pattern`);
Mosquitto 2.0.18 answering PUBACK 0x10, matched to its call through the packet id rumqttc's
`Outgoing::Publish` reports (MQTT `unhandled_pattern`, `drain`); librdkafka's `assign` from `Offset::End` without joining a
group (Kafka's streamed scenarios, which need the control topic); every spawned future being
`Send` (the crates compile). Still unverified: a produce to a missing topic with auto-create off
(the broker here auto-creates).

## Verification

Against known violations:

- Each fix in fix 2 was preceded by its failing run: `Ok(3000)` on NATS, Redis and MQTT, the 13 s
  hang on RabbitMQ.
- RabbitMQ before `holds_unserved`: `drain` failed with the client's `Timeout`.
- The held-event assertion on a false declaration (NATS): failed, as decision 1 records.
- Kafka's declared `recovery_after_disrupt` run with `--ignored`: failed with `Ok(3000)`.
- Kafka before fix 3: `two_instances` failed three of six runs, one reporting 9 of 10; the drain's
  held event was lost.

Three consecutive runs per target on the final tree, `cargo test -p <crate> --features integration
--test conformance` on stable (TCP and UDP without the feature), test time as libtest reports it:

| Target | Run 1 | Run 2 | Run 3 | Result |
| --- | --- | --- | --- | --- |
| `ulo-rpc-tcp` | 1.31 s | 1.31 s | 1.31 s | 20 passed |
| `ulo-rpc-udp` | 1.31 s | 1.30 s | 1.30 s | 19 passed, 1 ignored |
| `ulo-rpc-nats` | 2.40 s | 2.33 s | 2.37 s | 20 passed |
| `ulo-rpc-redis` | 4.34 s | 4.66 s | 7.30 s | 20 passed |
| `ulo-rpc-mqtt` | 4.66 s | 4.46 s | 4.46 s | 20 passed |
| `ulo-rpc-rabbitmq` | 11.99 s | 13.23 s | 12.31 s | 20 passed |
| `ulo-rpc-kafka` | 100.76 s | 100.39 s | 100.37 s | 19 passed, 1 ignored |

Container start is inside each scenario, so the times include it. Kafka's are mostly F1's teardown.
Flakes across every run of this batch: Kafka's `two_instances` before fix 3 (fix 3), NATS's drain
once (F3).

- `cargo test -p <link> --test conformance` without the feature: `running 0 tests` for all five,
  each file being `#![cfg(feature = "integration")]`.
- `cargo check -p <link> --all-targets --features integration` on stable, all five: no warning
  from the RPC crates; the 17 warnings printed are `crates/ulo/src`'s, the set listed in
  `batch2a-cfgattr.md`.
- `cargo test --workspace --no-fail-fast` without the feature: 350 passed, 0 failed, 61 ignored,
  446 s wall time.
