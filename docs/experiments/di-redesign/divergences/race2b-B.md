# Divergences: race 2b, area B (the broker links and the RPC conformance suite)

Every place the five broker links and `ulo-rpc-conformance` depart from `transports/DESIGN.md`,
the twentieth response, `REVIEW_2B.md` or the spine's frozen surface, or fill a shape those leave
open. Each entry gives what the design says, what was written, and why. All await the user's
sign-off.

No cargo was run. Every library call was read against its source in the registry (async-nats
0.46.0, redis 1.3.0, lapin 4.10.0 with amq-protocol 10.6.3, rumqttc 0.25.1, rdkafka 0.39.0); what
only a compile or a broker can show is under "Not verified".

Files written: `crates/ulo-rpc-{nats,redis,rabbitmq,mqtt,kafka}/{Cargo.toml, src/lib.rs,
src/link.rs}`, `crates/ulo-rpc-conformance/{Cargo.toml, src/lib.rs, src/cases/*.rs}`, the last
including a new `src/cases/app.rs`.

## Public surface added

```rust
impl RabbitMq { pub fn prefetch(self, prefetch: u16) -> Self; }          // entry 6
impl Kafka {
    pub fn topic_partitions(self, partitions: i32) -> Self;               // entry 10, B's to name
    pub fn replication_factor(self, replication: i32) -> Self;
}
// Cargo features: `ulo-rpc-redis/tls`, `ulo-rpc-kafka/tls` (entry 13)
```

Every other public item keeps the spine's signature. Each link struct gained `pub(crate)` fields
only its own crate reads.

## The wire on the brokers

### 1. A streamed request on a payload-carrying broker: a control lane and an `opened` handshake

- **Design:** §5.2 has the request lane carry the payload, `p` as the subject, queue or topic, `h`
  as native headers and `id` as the native correlation, "a body arriving without a reply address is
  an `evt` and one with it a `req`"; the reply lane carries `res`, `err`, `item`, `end`. Silent on
  `open`, `in`, `in_end` and `cancel` on NATS, AMQP, MQTT and Kafka.
- **Written:** `open` travels on the request lane as an empty body with a reserved header
  `ulo-t: open` (NATS header, AMQP header, MQTT user property, Kafka header) and the reply address.
  The server holds the call, answers on the reply lane with a header-only `ulo-t: opened`, then
  delivers `open`. `in`, `in_end` and `cancel` travel on one control lane per link, which every
  server instance reads outside the competing group: the subject `ulo.rpc.control` (NATS), the
  fanout exchange `ulo.rpc.control` with one auto-delete queue per instance (AMQP), the topic
  `ulo/rpc/control` outside `$share` (MQTT), the topic `ulo.rpc.control` assigned in full from its
  end (Kafka). Each control message carries the call's native correlation in the native slot (the
  reply subject on NATS, `correlation_id`, Correlation Data, `ulo-correlation-id`), and an instance
  delivers only those naming a call it holds. The client holds a streamed request's control frames
  in a per-call queue until `opened` arrives, then publishes them in order.
- **Why:** a competing group hands each request-lane message to one instance, so the items of a
  streamed request sent there would scatter across instances. The control lane reaches the instance
  that took the call; `opened` orders its items after the call is held there, which two lanes
  cannot otherwise promise. The cost: every instance receives every control message, and a client
  stream waits one round trip before its first item.

### 2. `cancel` of a unary or server-streamed call is best effort

- **Design:** "Dropping a stream or a pending request sends `cancel`".
- **Written:** the client publishes it on the control lane at once (no handshake precedes a `req`).
  A `cancel` that overtakes its `req`, possible because the lanes differ, finds no held call and is
  dropped; the call then runs to its end.
- **Why:** the old links documented the same race on their cancel channels. Closing it needs a
  handshake on every `req`, a round trip per call.

### 3. Frame ids: one local id per native correlation (R's request, R entry 2)

- **Written:** each server keeps a table from the native correlation to a link-local `u64`, from 1,
  delivers every frame under it, and releases the entry on the call's terminal reply (`res`, `err`,
  `end`) or on its `cancel`. The client recovers its own id from the correlation it chose
  (`<client id>.<call id>` on AMQP, MQTT, Kafka; `<inbox prefix>.<call id>` on NATS) and rewrites
  each reply frame's `id`, since the server wrote its local one. A control-lane `cancel` is
  delivered with the held call's reply path, so it reads as `ClientCancelled`; a broker link never
  loses one caller's connection apart from the others, so no link delivers `reply: None`.
  `ReplyPath::peer` is unused: a broker has no peer address.

### 4. Redis: the reply channel inside `h`, ids offset per caller, a control channel

- **Design:** Redis carries the whole frame, "replies on a per-client reply channel"; silent on how
  the server learns the channel and on caller id collisions.
- **Written:** a `req` or `open` frame carries the reserved header `ulo-reply` naming the caller's
  channel `ulo:rpc:reply:<uuid>`; the server strips it before the call's headers reach a handler.
  Every frame a caller sends has its id offset by a random `u64` base of its own (wrapping), which
  the client subtracts from each reply. `in`, `in_end` and `cancel` travel on `ulo:rpc:control`,
  so a drained server that unsubscribed its patterns still receives the items of a call it holds.
  No `opened` handshake: one publisher connection and one Pub/Sub connection deliver in order
  across channels.
- **Why:** frames carry headers but Redis Pub/Sub does not; `h` is the grammar's own slot. Callers
  allocating ids from zero would otherwise collide on the shared control channel.

### 5. Where a miss signal arrives after `send`, the link writes the `err`

- **Spine:** "A link with a miss signal fails `send` with `NoDestination`", mapped by `RpcClient`
  to `Unavailable` with `reason: "no_destination"`.
- **Written:** Redis (`PUBLISH` count of zero), RabbitMQ (`basic.return` through the confirm) and
  Kafka (`UNKNOWN_TOPIC_OR_PARTITION`, `UNKNOWN_TOPIC`) fail `send` with `NoDestination`. NATS's
  no-responders arrives as a 503 status message on the reply subject after the publish returned,
  and MQTT's PUBACK or PUBREC 0x10 after the packet left; both links answer the call on `replies`
  with an `err` frame of kind `unavailable`, `ErrorInfo { reason: "no_destination", domain:
  "ulo.rpc" }` (R entry 18's domain), message "nothing listens on pattern `p`".
- **Why:** awaiting the signal inside `send` would hold every call for a round trip, and on NATS
  there is no signal to await on success. The caller sees the same kind and reason.
- **Also:** the miss signal is reported for `req` and `open`. An `evt` is published without
  `mandatory` on AMQP and its `PUBLISH` count is not read on Redis, so an event to nowhere succeeds.

### 6. The AMQP prefetch is the link's own setting (R entry 7, routed by the coordinator)

- **Design:** §5.2, prefetch "set per consumer (`basic.qos` with `global = false`) to
  `max_inflight`'s `Max(n)`, and to 64 under `Default` or `Unlimited`".
- **Written:** `RabbitMq::prefetch(u16)`, 64 unset, 0 no limit as AMQP reads it, applied per
  consumer before each pattern's `basic.consume`. The server's `max_inflight` is not read.
- **Why:** the frozen `Link` gives a link no access to the server's settings: `prepare` takes the
  app, `listen` the patterns. **Open for the design:** whether `listen` (or a defaulted
  `Link::max_inflight(&mut self, Count)` called by `Server::prepare`) should carry the count, which
  removes the setting and restores the stated tie.

### 7. A RabbitMQ queue outlives its server

- **Written:** each pattern's queue is declared with default options, not auto-delete, not
  exclusive. A request sent while no instance consumes waits in the queue for the next one, so the
  caller sees its own `Timeout`, not `Unavailable`, though the link declares `miss_signal`;
  `basic.return` fires only for a pattern no server ever declared.
- **Why:** an auto-delete queue loses the requests it holds when its last consumer cancels, which
  the drain does; competing consumers picking up a restarted server's backlog is what the queue is
  for. The crate doc states it.

## Groups, drain and lifecycle

### 8. The group default, sanitised, and Kafka's too

- **Design:** §13.13, the root module's full type path on NATS and MQTT.
- **Written:** `format!("{:#}", app.root().name())` read in `prepare`, with whitespace, `*` and `>`
  replaced by `_` on NATS and whitespace, `/`, `+` and `#` on MQTT. A `.group(..)` the broker would
  refuse is a `prepare` failure naming the setting. Kafka's consumer group defaults to the same
  path, unsanitised.
- **Why:** a labelled or keyed root renders `label (path) @ Q #2`; NATS refuses whitespace in a
  queue name, MQTT `/` in a ShareName. Kafka's case is the same as NATS's, a fixed default
  (`master`'s `ulo-rpc-server`) letting two applications share a group.

### 9. The drain, and when the inbound stream ends

- **Design:** "NATS drain; AMQP `basic.cancel`; MQTT unsubscribe; Kafka pause"; Redis not listed;
  the inbound "ends once nothing more will arrive".
- **Written:** NATS drains each pattern subscription (`Subscriber::drain`); AMQP cancels each
  pattern consumer; MQTT unsubscribes each `$share` filter and resubscribes only the control topic
  after a reconnect while draining; Kafka pauses its assignment and commits asynchronously; Redis
  unsubscribes its patterns. The control lane stays open, and the inbound stream ends once draining
  holds no call (or at `close`), since a held call can still receive its items or a `cancel`.
  `close` closes the connections, aborts the event-loop tasks and commits synchronously on Kafka.

### 10. Kafka

- **Topic creation (B's open point):** `topic_partitions` and `replication_factor`, 1 each, for the
  handler topics created at `bind` and the client's reply topic; a topic that exists keeps its
  shape, and any other creation failure fails `bind`. The control topic is created with one
  partition.
- **Control consumer:** assigned every partition of `ulo.rpc.control` from its end, outside the
  group's rebalancing, with a `group.id` of its own that commits nothing; a subscription would read
  nothing until a first assignment seconds after `bind`. Partitions are read with `fetch_metadata`
  on a blocking thread.
- **`max_frame`:** declared `Some(1_000_000)`, the producer's `message.max.bytes` written out
  (librdkafka's default). Design: "size limit from broker configuration". A broker configured lower
  answers `MSG_SIZE_TOO_LARGE`, mapped to `FrameTooLarge` too.
- **Delivery:** `message.timeout.ms` is 30 s rather than five minutes. Offsets are stored once the
  handler settles the `Ack`, accepted or rejected, and committed by auto-commit
  (`enable.auto.offset.store = false`). The reply consumer's group is the reply topic's name, read
  from `earliest`, as on `master`.
- **Partition keys:** a request and its control frames are keyed by the client instance's id, a
  reply by its correlation id. The caller-supplied ordering key §5.3 names has no spelling on
  `RpcClient`, which is R's.

### 11. MQTT

- **CONNACK check (B's open point):** `listen` spawns the event loop and waits for its first
  outcome. A CONNACK whose Shared Subscription Available is 0 fails `bind` naming `Mqtt::group`; a
  SUBACK refusing any filter fails it naming the filter; a connection error before either fails it.
  After a reconnect the same CONNACK is logged at `error` and the link keeps the control topic.
- **Packet ids:** rumqttc's `publish` returns before a packet id exists. The client serialises its
  publishes and waits for each one's `Outgoing::Publish(pkid)` event before the next, so a PUBACK
  or PUBREC names its call. Sessions are clean; a connection lost before a publish leaves the
  client fails that `send` (`Unavailable` through `RpcClient`), and the packet ids in flight are
  forgotten.
- **URL:** parsed by the link (`mqtt`, `tcp`, `mqtts`, `ssl`; `user:password@`; bracketed IPv6):
  rumqttc's `parse_url` sits behind its optional `url` dependency and requires a `client_id`
  query. Each connection's client id is `ulo-` and 16 hex characters.
- **Limits:** the link accepts packets up to 268,435,455 bytes (rumqttc's default is 10 KiB); its
  `max_frame` is the CONNACK's Maximum Packet Size, known after the first connection.
- **Acknowledgment:** rumqttc acknowledges each incoming QoS 1 or 2 publish itself; the `Ack` is a
  no-op, as §5.2 says MQTT has nothing beyond the QoS acknowledgment.

### 12. NATS and MQTT declare `max_frame` once connected

- **Design:** NATS "maximum payload read from the server's `INFO`"; MQTT "maximum packet size from
  CONNACK".
- **Written:** `capabilities()` answers `None` until a connection has read the value, then the
  value. `send` refuses an oversized payload with `FrameTooLarge` once it is known.

### 13. TLS per link

- NATS: async-nats needs a crypto provider to compile at all, so the crate enables its `ring`
  feature and `tls://` works in every build. RabbitMQ (`amqps://`) and MQTT (`mqtts://`) use their
  libraries' default rustls features, already on through the workspace manifest. Redis: a `tls`
  feature enabling `redis/tokio-rustls-comp` and `tls-rustls-webpki-roots`; without it redis refuses
  `rediss://` when `prepare` parses the url. Kafka: a `tls` feature enabling `rdkafka/ssl` (system
  OpenSSL); an `ssl://host:port` broker entry sets `security.protocol=ssl`, and without the feature
  `prepare` refuses it. Mixing `ssl://` and plaintext entries is refused.

### 14. Smaller choices

- A request-lane message of an unknown `ulo-t` kind is rejected without requeue on AMQP and Kafka
  and dropped with a `warn` on NATS, MQTT and Redis.
- AMQP headers are a field table, so a header name repeated in `CallHeaders` keeps its last value;
  incoming tables, arrays and byte arrays are left out of `CallHeaders`.
- Links connect at `bind` without retrying: an unreachable broker fails `bind` with
  `StartupError::Bind`. `master`'s retry-on-initial-connect loops are not carried over. After
  `bind`, NATS and lapin recover by themselves, and the Redis, MQTT and Kafka links reconnect in
  their read loops.

## The conformance suite

### 15. Shape of the suite

- **Written:** `cases/app.rs` (a new file inside `cases/*.rs`) holds the application: three
  controllers, `CoreController` always, `StreamController` where the link's `shapes` carry every
  streamed shape, `BinaryController` where it declares `binary`, so a link refusing a shape or a
  payload kind in `prepare` still runs every scenario it carries. Each scenario starts its own
  broker through `Broker::start`, one server and one client app (`RpcClientModule` imported over
  `Broker::link()`), and waits for a unary call to succeed within `Budget::boot`. A server's
  handlers record into a `Probe` bound by value in its root module.
- **Excluded capabilities:** a streamed scenario on a link without the streamed shapes asserts
  that a server mounting them is refused at startup with `StartupError::Configure`; the binary
  scenario on a JSON link asserts the client's `binary_unsupported` and the startup refusal of a
  binary handler.
- **Misses:** `Unavailable` with `pattern_unhandled` or `no_destination` where `miss_signal`, the
  client's `Timeout` otherwise, as §5.2 states. The unhandled-event scenario cannot see a broker's
  acknowledgment, since a broker never delivers an event for an unsubscribed pattern; it asserts the
  emit fails as `Unavailable` or not at all and that the server still answers.
- **Cancel and deadline:** the ticking stream and the stalled handler each hold a value that
  records its execution's `cancel_reason()` when dropped, so the scenarios assert
  `ClientCancelled` and `Deadline` whether the server drops the handler's future or lets it finish.
  `deadline-ms` is set as a header (R's client keeps a caller's own value unless `.within(..)`).
- **Oversized:** with a declared `max_frame`, one byte over it answers `payload_too_large`. Without
  one (NATS and MQTT before connecting, Redis, AMQP), an 8 MiB payload must either round-trip or
  answer `payload_too_large`.
- **Drain:** a held call and a stream that ends on `draining()` are open when the server closes; a
  new call after the drain starts is `Unavailable`, or the caller's `Timeout` on a link that is not
  `Addressed` (entry 7, Kafka's pause, a link without a miss signal).
- **Two instances:** ten events; `Competing` delivers ten across both, `FanOut` twenty. Under
  `Addressed` a second server cannot share the address, so one instance runs and receives all ten.
- **`Broker` docs:** `start` must answer a broker or namespace no other scenario shares, since the
  stamped tests run in parallel with the same patterns, groups and control lanes.

## Replaced from `master`, and not carried

- The old adapters' and client transports' wire (`{"response":..}`, `{"stream":..}`, the
  `ulo.rpc.cancel` channels and their JSON notices) is replaced by the grammar of §5.2 and the
  control lanes of entry 1. `IntoNatsServers` is gone: `Nats::url` takes a comma-separated list.
- Not written, under rule 2: the per-crate `tests/conformance.rs` and the `integration` features
  the old crates gated them behind; the old examples.

## Requests for other areas

- **Design (via the coordinator):** entry 6, whether the link SPI carries `max_inflight`.
- **Coordinator, workspace manifest:** none required. The links add member-level features to
  workspace dependencies (`async-nats/ring`, `uuid/v4`, `bytes/serde` in the suite, the `tls`
  features), which members may do. Note: the workspace keeps rumqttc's and lapin's default
  features, which pull their own rustls providers.

## Not verified

- Nothing was compiled or run against a broker. Specifically: rustc accepting every spawned future
  as `Send` (the Kafka lanes detach each `BorrowedMessage` inside the `select!` arm for that
  reason); `ToRedisArgs` for `&[String]` and `&Vec<String>` in `subscribe`/`unsubscribe`; lapin's
  confirmation carrying the returned message for a `mandatory` publish on a channel that also
  consumes direct reply-to; direct reply-to working under confirm mode at all; rumqttc emitting
  `Outgoing::Publish` for every publish, QoS 0 included, in request order; librdkafka's `assign` on
  a consumer created with a `group.id` reading from `Offset::End` without joining the group; a
  produce to a missing topic with auto-create off failing with `UNKNOWN_TOPIC_OR_PARTITION` rather
  than timing out (B's open point, still open); `rd_kafka_offset_store` storing `offset + 1`.
- The suite's macros and handlers depend on R's `#[ulo_rpc::message]`/`#[event]`, its reply probe
  (R entry 1: `Result<(), E>` answered as no payload, `Bytes` as a serde value) and its client's
  handling of an empty `res` for `request::<_, ()>`.
