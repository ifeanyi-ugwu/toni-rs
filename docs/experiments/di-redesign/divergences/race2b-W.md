# Divergences: race 2b, area W (WebSocket)

Every place area W departs from `transports/DESIGN.md` §3.5 and §4, `DESIGN.md`, the twentieth
response, `REVIEW_2B.md` or the spine's frozen surface, or fills what they leave open. Each entry
gives what the design says, what was written, and why. All await the user's sign-off.

Files written: `crates/ulo-ws/{Cargo.toml, src/lib.rs, src/transport.rs, src/gateway.rs,
src/session.rs, src/envelope.rs, src/codec.rs, src/connection.rs, src/handoff.rs, src/server.rs,
src/rooms.rs, src/broadcast.rs, src/module.rs, src/__private.rs}`,
`crates/ulo-ws-macros/src/{gateway.rs, message.rs}`, `crates/ulo-ws-redis/{Cargo.toml, src/lib.rs}`.
No file outside the area was edited. No cargo was run.

## The frozen contracts, as kept

`Gateway`, `GatewayConfig`, `GatewaySettings`, `Session<T>`, `SessionHandle`, `ConnectRefused`,
`Frame`, `Reply`, `WsModule`, `Connection` and every other public signature the spine wrote keep
their names, fields and signatures. Additions, each logged below: the public field
`GatewaySettings::port` with its setter and the enum `Port` (entry 2), `impl FromCall<Ws> for WsCx`
(entry 23), `impl IntoReply<Ws>` for `Frame` and `Reply` (entry 1), `impl Validate for Payload<T>`
(entry 23), and a `#[diagnostic::on_unimplemented]` on `GatewayConfig` (entry 27). Private fields
were added to `WsCx`'s and `ConnectCx`'s inner structs, `ConnectionInfo` (`id`), `UpgradeHead`
(`subprotocol`), `Rooms` (`hub`), `InMemory`, `Server` and `ulo_ws_redis::Redis`.

## Points the spine left to W

### 1. `IntoReply<Ws>`: a WebSocket reply probe with a marker-typed value side

- **Design:** §4.1, `IntoReply<Ws>` for `()`, `T: Serialize`, `Frame` and
  `S: Stream<Item = Result<T, E>>`.
- **Written:** `IntoReply<Ws>` is implemented for `Frame` and `Reply` only. The serializable values
  and the streams are answered through a doc-hidden trait `__private::Answer<M>`, implemented three
  times under three markers: `ViaReply` for any `V: IntoReply<Ws>`, `ViaStream` for a stream of
  `Result<T: Serialize, E: Into<CallError>>`, `ViaSerde` for any `T: Serialize`. `()` takes the
  serde arm and answers `Reply::None`, recognised by `TypeId`. `#[message]` hands
  `ulo-handler-codegen` a `Paths` whose `transport` is `::ulo_ws::__private::reply`, a module
  re-exporting `ulo-transport`'s `Param` and `controller` beside W's own `IntoReplyProbe` and its
  three arms `ViaCallError<M>`, `ViaBoxError<M>`, `ViaValue<M>`, each bounding `V: Answer<M>`; the
  codegen is unchanged.
- **Why:** `impl<T: Serialize> IntoReply<Ws> for T` is refused by the orphan rule before coherence
  is reached: `T` is an uncovered type parameter ahead of the local `Ws` (E0210), and the stream
  blanket likewise. Distinct marker parameters make the three blankets three traits, so they do not
  collide (E0119), and the probe infers `M` from the one impl whose where-clauses the concrete
  value meets, the mechanism `Param<T, M>` already relies on. An autoref ladder alone could not
  carry it: the codegen writes the probe with three references, which gives four ranks, and the
  error side times the value side needs nine. Area R chose the same shape for `IntoReply<Rpc>`
  (`race2b-R.md` entry 1).
- **Limits:** a type both `Serialize` and a stream, or both `Serialize` and a user's own
  `IntoReply<Ws>`, is ambiguous at the handler (E0283); `IntoReply<Ws> for ()` is not written, since
  it would make every `()` answer ambiguous.

### 2. Which gateways each server serves: `GatewaySettings::port`

- **Design:** silent; the spine noted that a gateway on both ports needs a selector.
- **Written:** `pub enum Port { Http, Own }` (default `Http`), the public field
  `GatewaySettings::port`, the setter `.port(..)` and the attribute argument `port = http | own`. The
  hand-off `WsModule` registers serves the `Http` gateways on the HTTP server's port and on an
  embedding host declaring `upgrades`; `ulo_ws::Server` serves the `Own` gateways, and its
  `prepare` refuses a server with none. No gateway is served on both.
- **Why:** serving every gateway on both ports would expose a gateway on a port its author did not
  choose, and neither server can learn at `prepare` whether the other is bound. A field on the
  `#[non_exhaustive]` settings with a setter keeps Q's `GatewaySettings::at(..)` calls compiling, and
  Q's gateway defaults to the HTTP port it is meant for. `master` decided the same by intent
  (`port = N` on the gateway).
- **Not checked:** a gateway declared `port = own` in an app that binds no `ulo_ws::Server` is
  reachable by nothing, and nothing reports it.

### 3. A queued control frame reaches the wire through an explicit flush (open point)

- **Design:** §4.2, "the gateway loop polls the connection after queueing a control frame".
- **Written:** tungstenite queues the Pong for a Ping and the reply to a Close, and writes them only
  on the next read or flush. The read loop calls `flush()` right after reading a Ping or a Close,
  since it stops reading under `max_inflight` and in the drain. After a Close in either direction
  the loop keeps reading, data frames ignored, until the stream ends, bounded by `pong_timeout`
  (unbounded under `Bound::Unbounded`, until `close`).
- **Why:** a later read is not guaranteed to come; `pong_timeout` is the configured time a peer is
  given to answer a control frame.

### 4. `Error::Capacity` is `ProtocolError`, and W sends the close codes tungstenite does not (open point)

- **Design:** §4.2, over `message_limit` "the connection closes with 1009, which tungstenite answers
  itself when configured through `WebSocketConfig::max_message_size`"; the twentieth response maps
  capacity errors to `ProtocolError`.
- **Written:** tungstenite 0.28 returns protocol, UTF-8 and capacity errors from `read` without
  writing a Close (read `tungstenite-0.28.0/src/protocol/mod.rs`, `read_message_frame`), and fuses
  the stream. The loop writes the Close itself: 1002 for `Error::Protocol`, 1007 for `Error::Utf8`,
  1009 for `Error::Capacity`, each `DisconnectReason::ProtocolError`.
  `ProtocolError::ResetWithoutClosingHandshake` is `Lost`, a connection closed without a frame. The
  frame limit is `min(16 MiB, message_limit)`.
- **Why:** the design's sentence about tungstenite does not match the crate it names.

### 5. A same-port gateway's defaults come with the hand-off (open point)

- **Written:** `WsModule::register` builds the `Handoff` with the module's `Defaults` and registers
  it in `Upgrades`; the hand-off's `prepare` builds its gateway table with them and refuses their
  zero limits, as `ulo_ws::Server::prepare` does with the server's own.

### 6. Prefix join (request from Q)

- **Design:** `.at(prefix)` applies "to every route and gateway path of that controller", silent on
  the join.
- **Written:** the served path is the connect handler's `HandlerInfo::prefix()` joined to
  `GatewaySettings::path` by `ulo_http`'s `Pattern::join` rule: one `/` between them, the prefix's
  trailing slash dropped, so `.at("/graphql")` with path `/` serves `/graphql`. Paths match as
  written, one trailing slash insignificant; a `{param}` segment is refused in `prepare`.

## Filled gaps and departures

### 7. The answer to a message with an `id` whose handler returns nothing

- **Design:** silent on `()` with an `id`.
- **Written:** `{"id":..,"complete":true}`, the end marker of a stream with no items.
- **Why:** a client that sent an id waits for an answer; `complete` resolves it without inventing a
  `data` value.

### 8. A streamed answer to a fire-and-forget message

- **Design:** "a message without an id is fire-and-forget: success sends no ack".
- **Written:** its items and its `complete` are written without an `id`.
- **Why:** items are not an ack.

### 9. `cancel`

- **Written:** `ClientCancelled` on every message in flight with the id; a cut stream writes
  nothing more. A `cancel` without an `id` answers `bad_request`. An unknown id is ignored.

### 10. Defaults the design leaves open

- **Written:** `max_inflight` 64, `max_outbound` 1024, `max_connections` unbounded at `Default`;
  `message_limit` 64 MiB and keep-alive 30 s as designed.

### 11. What `max_inflight` counts, and how a stream meets `max_outbound`

- **Written:** a message counts against `max_inflight` until its handler's answer is known; a
  streamed answer still writing does not hold the slot, but the drain waits for it. Stream items
  wait for room in the outbound queue; the overflow policy governs what the gateway cannot hold
  back: replies of one frame, error envelopes, broadcasts and `Connection::send`.
- **Why:** a long-lived stream holding a slot would stop the connection reading, and a `cancel` for
  it could not be read. A stream larger than `max_outbound` would trip "slow consumer" on its own.

### 12. A text frame on a MessagePack gateway closes with 1003 too

- **Design:** "binary frames on a text-only gateway close with 1003".
- **Written:** each codec's other frame kind closes with 1003, `ServerClose { code: 1003 }`. A
  hand-written gateway receives both kinds.

### 13. `ConnectRefused::kind` for the kinds the design does not map

- **Written:** `BadRequest`, `NotFound`, `Conflict`, `Unprocessable` close with 1008 like
  `Unauthorized` and `Forbidden`; `Timeout`, `Unimplemented` and any later kind with 1011. A reason
  over 123 bytes is cut at a character boundary.

### 14. `refuse = handshake`'s 401 carries `WWW-Authenticate: Bearer`

- **Why:** RFC 9110 requires a challenge on a 401, and the HTTP server's configured challenge is
  not reachable from the hand-off.

### 15. Hand-written gateways

- **Written:** the instance is resolved once per connection in the connection phase's execution
  and kept. Messages reach `on_message` one at a time, in order, on a task of their own; one queued
  or running counts against `max_inflight`. A panic in `on_message` is logged and closes 1011. A
  hand-written gateway that also declares `#[message]` handlers is refused in `prepare`. In the drain
  such a connection is idle whenever no `on_message` runs and closes 1001 at once; the executions it
  opened are its own, and `on_disconnect` receives `Drain`.
- **For Q:** an operation's `complete` written after the drain began races the 1001 and is dropped
  once the close is queued.

### 16. `after_init` and `on_disconnect` resolve the gateway in an execution

- **Written:** `after_init` resolves the gateway in an execution of its module that ends before the
  hook runs, so a long hook holds no execution across the drain. `on_disconnect` opens a plain
  execution while the app serves and a terminal one, with the token the drain handed the server or
  the hand-off, once `Execution::open` is refused. Room membership is removed after the hook, so it
  reads `conn.rooms()`.

### 17. Sessions

- **Written:** a `session_with` factory runs in the connection phase's execution after
  `ConnectionInfo` and `UpgradeHead` are seeded; its parameters are the connect handler's
  `dependencies`, so `wire()` checks them. A factory failure refuses the connection with 1011 and is
  logged. `session_with = |..| expr` is wrapped in `async move` by the attribute unless the body is an
  `async` block. A gateway with no session, or one of another type, answers `Session<T>` as an input
  never seeded (`LookupError::NotFound`, `LookupKind::Input`, naming `Session<T>`): `KeyName` has no
  public constructor, so no `WrongType` can be built outside the core.

### 18. Broadcasts travel as JSON between processes

- **Design:** the adapter's frame is "an encoded envelope", each gateway's codec applied.
- **Written:** `Broadcast::emit` publishes `{"event","data"}` as JSON; each process re-encodes it per
  gateway on delivery, with that gateway's event field and codec. A MessagePack gateway receives the
  MessagePack form of the JSON value, so a byte string arrives as an array of numbers.
- **Why:** one publish serves gateways of both codecs and either event field.

### 19. The Redis adapter's channels

- **Written:** `ulo:ws:room:<room>`, `ulo:ws:node:<node>` (a client, published to the process its id
  names), `ulo:ws:all`; each message the target as JSON, a newline, then the frame. A process
  subscribes to its node's channel and `ulo:ws:all` and pattern-subscribes `ulo:ws:room:*`, since the
  adapter SPI learns of no membership change; rooms are filtered on delivery. A lost subscription
  reconnects after one second; a failed publish drops the cached connection. `rediss://` needs
  redis's `tokio-rustls-comp` feature, enabled in `ulo-ws-redis`'s manifest.

### 20. `NodeId::current`

- **Written:** two `RandomState` hashes over the process id and the clock, once per process.
- **Why:** no random-number crate is a dependency.

### 21. A MessagePack message's `id`

- **Written:** kept as the JSON text of the scalar it decoded to and written back as that scalar:
  equal by value, not by bytes. JSON ids are echoed byte for byte through `serde_json`'s
  `raw_value`, enabled in `ulo-ws`'s manifest; `data` is kept as raw JSON until a `Payload<T>` reads
  it. `"data": null` is present (`Payload<Option<T>>` reads `None`); an absent key is `Missing`.

### 22. The `Payload` probe records nothing

- **Written:** `WsHandler::payload(..)` is kept as a no-op and `#[message]` emits no probe call.
- **Why:** the data stays undecoded until read, so no handler needs marking, and the probe cannot see
  `Valid<Payload<T>>`, `Option<Payload<T>>` or a custom extractor delegating to `Payload`.

### 23. Additive impls

- `impl FromCall<Ws> for WsCx`, as `HttpCx` is a parameter on HTTP; `impl Validate for Payload<T>`, so
  `Valid<Payload<T>>` checks the `T` as `Valid<Json<T>>` does.

### 24. `WsModule` is global

- **Written:** `register` calls `m.global()`, binds `Rooms` and the configured adapter under a private
  key `also_as` `dyn BroadcastAdapter` (both exported), and writes the application's `Hub` as module
  metadata, which `ulo_ws::Server::prepare` reads through `module_meta`. An application binding the
  standalone server without importing `WsModule` gets an in-memory hub of the server's own.
- **Why:** `Dep<Rooms>` resolves from every module, and the standalone server finds the hub
  whichever module imported `WsModule`.

### 25. Refused in `prepare` beyond §12

- The event field `id` or `data`, which the envelope's own keys would shadow; a gateway path not
  starting with `/` or holding a `{param}`; two handlers for one event; message handlers on a
  hand-written gateway; a standalone server with no `port = own` gateway. Each failure names the
  gateway's controller through `Failure::naming`.

### 26. The handshake's refusals

- A method other than GET answers 405 with `Allow: GET`; a missing `Connection: Upgrade`,
  `Upgrade: websocket` or `Sec-WebSocket-Key` 400; a version other than 13 426 with
  `Sec-WebSocket-Version: 13`; a request during the drain 503. A non-upgrade request on a standalone
  gateway path is one of these. On the HTTP port a request whose backend handed no upgrade future
  answers 400.

### 27. `GatewayConfig` carries a diagnostic

- A `#[message]` handler on an impl without `#[ulo_ws::gateway]` fails E0277 with a note naming the
  attribute.

### 28. The `max_connections` slot

- Under `refuse = close` it is taken after the 101 and before the connection phase; under
  `refuse = handshake` after admission, so a refusal counts against no limit. Over the limit the
  connection closes 1013 before any hook runs.

### 29. A streamed answer's `Completed` is reported once its `complete` is queued

- **Design:** §2.6, `Completed` when the transport finished writing.
- **Written:** `Tracked` drops after the `complete` envelope is queued, not written.

## Retired

`ulo-ws-tungstenite` is replaced by `ulo_ws::Server`; its directory stays in the root manifest's
`exclude` until the coordinator removes it. `master`'s `ulo::ws`, `BroadcastService` and
`ulo-ws-redis`'s Redis-set membership are not carried over (§4.3, R10).

## Requests for other areas

None. `crates/ulo-ws-redis/Cargo.toml` enables redis's `tokio-rustls-comp` and tokio's `time` on
workspace dependencies, and `crates/ulo-ws/Cargo.toml` enables `serde_json`'s `raw_value`,
`futures-util`'s `sink` and adds `http-body-util`; the root manifest is unchanged.

## Not verified

- Nothing was compiled.
- The reply probe's method lookup inferring `M` from `V: Answer<M>` across the three arms.
- tokio's `select!` branches in the read loop borrowing `ws`, the outbound queue and the timers
  disjointly, and the read loop's future being `Send` for `tokio::spawn`.
- `ulo::dispatch`, `dispatch_late` and `recover` futures being `Send` inside a spawned connection.
- redis 1.3's `Cmd::query_async::<i64>` and `PubSub::into_on_message` under the workspace's features.
