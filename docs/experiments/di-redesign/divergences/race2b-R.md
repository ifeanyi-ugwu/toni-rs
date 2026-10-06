# Divergences: race 2b, area R (RPC core and the socket links)

Every place area R departs from `transports/DESIGN.md` §5, `DESIGN.md`, the twentieth response,
`REVIEW_2B.md` or the spine's surface (`race2b-S.md`), or fills a shape they leave open. Each entry
gives what the design says, what was written, and why. All await the user's sign-off.

Files written: `crates/ulo-rpc/{Cargo.toml, src/lib.rs, transport.rs, frame.rs, codec.rs, link.rs,
server.rs, dispatch.rs, client.rs, client_module.rs, extract.rs, __private.rs}`,
`crates/ulo-rpc-macros/src/{message.rs, event.rs}`, `crates/ulo-rpc-tcp/src/{lib.rs, link.rs}`,
`crates/ulo-rpc-udp/src/{lib.rs, link.rs}`. Every `todo!()` in them is replaced.

No cargo and no git were run. `master`'s TCP and UDP crates were not read: reading them needs a
git command, which the race rules forbid. What `master` had that this does not carry over is
therefore not recorded beyond what `CLAUDE.md` lists (`with_retries`, `with_retry_backoff` and
`with_drain_timeout` on UDP and TCP, `with_max_inflight` on the link; see entry 27).

## The reply probe (the spine's open point)

### 1. `IntoReply<Rpc>` for a bare value goes through `ulo-rpc`'s own probe

- **Spine:** `IntoReply<Rpc>` for `T: Serialize` collides with the other answer impls (E0119); R
  picks the mechanism.
- **Written:** the blanket is refused before coherence is reached: `impl<V: Serialize>
  IntoReply<Rpc> for V` has an uncovered `V` ahead of the local `Rpc`, which the orphan rule refuses
  (E0210), and `ulo-transport`'s probe asks `V: IntoReply<Rpc>` on its value arm. So the RPC
  attributes build `ulo_handler_codegen::Paths` with `transport: ::ulo_rpc`, and the generated call
  names `::ulo_rpc::__private::{Param, controller}` (re-exported from `ulo-transport`) and
  `ulo-rpc`'s own `IntoReplyProbe` with traits named `ViaCallError`, `ViaBoxError`, `ViaValue`.
  Its three autoref arms rank the error side as `ulo-transport`'s do. The value side is
  `Answer<M>`, a marker parameter inferred at the call as `Param<T, M>` is:
  - `AsReply`: any `IntoReply<Rpc>`, so `Reply`, `Data` and a user's own impl.
  - `AsData`: any `Serialize`, encoded by the link's codec as one `res`. `()` takes this arm and is
    answered as no payload, told apart by `TypeId`.
  - `AsStream<M>`: `Stream<Item = Result<U, E>>` with `U: Serialize`, `E: ItemError<M>`, where
    `ItemError` is `Into<CallError>` (`ItemCall`) or `BoxError` itself (`ItemBoxed`), disjoint since
    a `BoxError` is no `Classify` error.
- **Why:** markers alone cannot rank the error side, where `CallError` and `BoxError` overlap, and
  autoref alone cannot rank the value side inside one generic impl. The codegen's `Paths` fields are
  public for a user-written transport, so pointing `transport` at the transport crate stays inside
  its contract, though `Paths::transport`'s doc says "`ulo-transport` as the transport crate
  re-exports it".
- **Limits:** a type implementing both `IntoReply<Rpc>` and `Serialize`, or a named stream type that
  is also `Serialize`, is ambiguous and fails with "type annotations needed". A stream of bare items
  (`Stream<Item = Invoice>`) is not an answer: it would overlap `AsStream` wherever the error type
  is `Serialize`. A stream item's error of another type (`io::Error`) needs a `map_err`.
- **For W:** the spine left W the same question for `IntoReply<Ws>`; this mechanism carries over.

## The link contract (frozen, read by B)

### 2. Delivery ids are unique per inbound stream

- **Design:** silent on how the server correlates `in`, `in_end` and `cancel` with a call.
- **Written:** a doc on `link::Inbound`: ids are unique among the calls in flight on one inbound
  stream. TCP and UDP map each caller's id to one of their own and back on the reply path.
- **Why:** TCP ids are per connection and the server sees one merged stream, so two connections
  would collide on id 1. **Request to B:** each broker link allocates one id per native
  correlation (`_INBOX` subject, `correlation_id`, Correlation Data, reply header).

### 3. A lost caller is `cancel` with `reply: None`

- **Design:** `Disconnected` fires when the peer closes the connection; the grammar has no frame for
  it.
- **Written:** a doc on `Delivery`: a link that loses the caller of calls in flight delivers
  `cancel` for each with `reply: None`, read as `Disconnected`; a caller's own `cancel` carries its
  reply path and is `ClientCancelled`. TCP does this when a connection closes.
- **Why:** the frozen `Delivery` cannot gain a field, and the reason matters for
  `StreamOutcome::CutOff`.

### 4. `ReplyPath::peer(self, SocketAddr)`, and an event may carry a reply path

- **Design:** `LinkInfo::peer` is the caller's address on TCP and UDP; `Delivery` carries none.
- **Written:** `ReplyPath` gains a private `peer` and the public builder `peer`; the server reads it
  for `LinkInfo`. `Delivery::reply`'s doc allows a reply path on an event to carry the peer, which
  the server never replies on. TCP and UDP attach one to every delivery.
- **Why:** additive on a frozen type; B's links need not call it.

### 5. A reply send failing with `FrameTooLarge` is replaced by `err` `internal`

- **Design:** silent on a reply over the link's limit.
- **Written:** `ReplyPath::send`'s doc and the dispatcher: on `FrameTooLarge` the server sends `err`
  of kind `internal` with the same id; any other send failure cancels the call `Disconnected`.

### 6. The codec comes from `Capabilities::binary`

- **Design:** the server decodes payloads and encodes replies in the link's codec; neither `Link`
  nor `Capabilities` names it.
- **Written:** `Codec::of(&Capabilities)` (crate-private): `binary: true` is CBOR, `false` JSON.
- **Why:** the codec set is closed. A third codec would need a `Capabilities::codec` field.

### 7. The AMQP prefetch has no path from `max_inflight` to the link

- **Design:** the RabbitMQ link's per-consumer prefetch follows `Server::max_inflight`.
- **Written:** nothing: `Link::listen` takes the patterns alone. **Open point for B and the
  coordinator:** the RabbitMQ link reads its own builder setting, or `listen` gains the count.

## Core and server

### 8. `ulo-rpc` depends on tokio; `bytes` gains `serde`

- **Written:** `tokio` with `rt`, `sync`, `macros` (per-call tasks in a `JoinSet`, the client's
  reply router, channels, `select!`); `bytes` with `serde`, so `Bytes` and `Payload<Bytes>` travel as
  a CBOR byte string.
- **Why:** principle 5 allows a transport crate tokio. One task per call keeps calls on every core.
  **Request to the coordinator:** none; both are member-level features.

### 9. `FromCall<Rpc> for Bytes`

- **Design:** "`Payload<Bytes>` and `Bytes` answering `PayloadKind::Binary`".
- **Written:** a bare `Bytes` parameter is the payload decoded as `Payload<Bytes>` is.

### 10. `IntoReply<Rpc>` is implemented for `Reply` and `Data`, not for `()`

- **Why:** entry 1; `()` as `IntoReply<Rpc>` would make every `()` answer ambiguous.

### 11. Shape detection

- **Design:** an `Inbound<T>` parameter is a streamed request, a stream return a streamed reply.
- **Written:** the request side is an autoref probe per parameter (`InboundProbe`), so an alias of
  `Inbound` counts. The reply side is read from the return type as written: an `impl` or `dyn`
  bound naming `Stream` or `TryStream`, or a path segment `BoxStream`/`LocalBoxStream`, anywhere in
  it. A stream behind an alias is declared unary; on a link without streamed shapes the server then
  answers it `err` `internal` ("the udp link carries no streamed reply") at runtime.
- **Why:** an opaque return type cannot be named at mount, where `HandlerSpec::shape` is set.

### 12. Two new refusals for `#[event]`

- **Written:** a stream return on `#[event]` is a compile error at the return type; an `#[event]`
  whose shape is not unary (an `Inbound<T>` parameter) is refused in `prepare`.
- **Why:** an event is one payload answered by nothing. Not in §12's table.

### 13. Mismatched frames

- **Design:** "`req` sent to a streamed-request pattern answers `bad_request`".
- **Written:** `req` or `evt` to a streamed-request pattern, and `open` to a single-payload or event
  pattern, are offered to the error handlers through `recover(Some(handler))` as `BadRequest`. A
  `req` to an `#[event]` handler runs it and answers an empty `res`; an `evt` to a `#[message]`
  handler runs it and drops the reply.
- **Why:** `nats req user.created` reaches an event handler as a `req` (R20).

### 14. Event acknowledgment

- **Design:** acknowledged once the handler completes; an unhandled event rejected.
- **Written:** acknowledged on `Ok`; rejected without requeue on an error or a passed deadline;
  left unsettled when shed, refused during the drain, or cancelled otherwise, so a broker redelivers
  it. An unhandled event is counted in a server-local counter that only its log line reads.

### 15. A passed `deadline-ms` renders `timeout` directly

- **Design:** "`deadline-ms` firing `Deadline` and rendering `timeout`".
- **Written:** the execution is cancelled `Deadline` and `err` `timeout` is written without
  offering the error handlers a `Timeout`, unlike HTTP's route timeout. A `deadline-ms` that does
  not parse as a `u64` sets no deadline. A cancel for `ClientCancelled` or `Disconnected` writes
  nothing; `Drain` and `Explicit` write `err` `unavailable`.

### 16. The streamed request's items are unbounded

- **Written:** `in` items reach the handler over an unbounded channel.
- **Why:** a bound needs a window, which is the reserved `credit`; blocking the serve loop would
  stall every call on the link behind one slow handler. `max_frame` bounds each item.

### 17. The shed answer carries `RetryAfter`

- **Written:** over `max_inflight`, `err` `unavailable` with a `RetryAfter` detail of one second,
  `Admission`'s default. `Server` has no `shed_retry_after` setting.

### 18. `ErrorInfo` domain `"ulo.rpc"`

- **Design:** names the reasons, not the domain.

### 19. `.at(prefix)` does not apply to patterns

- **Design:** the prefix applies "to every route and gateway path".
- **Written:** RPC patterns ignore `HandlerInfo::prefix`.

### 20. `serve` and `close`

- **Written:** `serve` spawns each call into a `JoinSet` and routes `in`, `in_end` and `cancel`
  synchronously in arrival order. When the inbound stream ends before the drain it returns `Err`,
  which starts the shutdown; after the drain it waits for its calls. `close` raises a signal on
  which `serve` aborts its calls, then calls `Link::close`. A handler value not built by the RPC
  attributes is refused in `prepare`.

## Frame grammar and codecs (R's open points)

### 21. The `credit` frame

- **Written:** `{t:"credit", id, n}`, `n` the further items the sender of `credit` grants for stream
  `id`. Reserved: the server ignores it and no link sends it.

### 22. Wire details the grammar leaves open

- **Written:** a frame is a map, `t` first. `h` is a map in order, a repeated name written once per
  value and read back so. An empty payload is written as no `d`, and a missing or `null` `d` reads as
  empty, which decodes as `null`. `e.details` is `Details`' stable JSON, parsed back by tag, an
  unknown tag kept as `Detail::Json`. An unknown `kind` reads as `internal`; unknown members are
  ignored.

### 23. The client's binary refusal

- **Written:** on a JSON link a payload is serialized through a `serde_json` formatter that notes
  `write_byte_array`; one that wrote raw bytes is refused `BadRequest`/`binary_unsupported` before
  any I/O. `Codec::encode` itself still writes bytes as an array of numbers.

### 24. Redis's whole-frame carriage shares `Codec::encode_frame` and `decode_frame`

- **Plan:** "how the Redis whole-frame carriage shares code with TCP's".
- **Written:** the shared part is the public `Codec::{encode_frame, decode_frame}`; TCP adds only
  its 4-byte prefix, so a Redis link publishes `encode_frame`'s bytes as the message and reads them
  back with `decode_frame`. The id mapping of entry 2 is per link.

## Client

### 25. Client behaviour the design leaves open

- **Written:**
  - One reply-router task per connection; a reply for an id no call waits on is dropped, which is
    the `FanOut` duplicate rule.
  - A lost reply lane fails the calls waiting on it `Unavailable` and the next call connects again;
    a `goaway` keeps the calls in flight and sends the next call over a new connection.
  - `cancel` on drop is sent from a spawned task; dropped outside a tokio runtime it is not sent.
  - `.within(&exec)` stamps `deadline-ms` with the remaining time (replacing one the caller set),
    refuses a passed deadline before I/O as `Timeout`, and bounds the wait by the shorter of the
    timeout and the remaining time. The execution's cancellation fails the call `Timeout` for
    `Deadline` and `Unavailable` otherwise.
  - A stream call answered by a single `res` yields that value and ends.
  - A failure in a streamed request's items fails the call and sends `cancel`.

### 26. `RpcClientModule`

- **Design:** `timeout(Bound::After(ZERO))` refused "in `prepare`"; identity unstated.
- **Written:** a module has no `prepare`, so the zero timeout is recorded through
  `ModuleDef::try_value` and `wire()` reports it. The identity is a per-`for_root` instance labelled
  `RpcClientModule`, so two clients of one link type are two modules. The client is a singleton
  reading `Dep<dyn Timer>`, so an app without a `Timer` fails `wire()`. The crate-private `link`
  field became `Arc<L>`, and an `instance` field was added.

## TCP and UDP

### 27. TCP

- **Written:**
  - `max_frame` defaults to 4 MiB (R's open point), twice HTTP's default body limit, capped at the
    4-byte prefix's range; `max_frame(0)` is refused in `prepare`.
  - A frame that does not decode drops the connection: without its id nothing can be answered.
  - The drain closes the listener and sends `goaway` with `try_send`; a connection whose write
    queue is full receives none. Connections stay open for their calls until they close or the link
    closes.
  - The TLS handshake has no timeout of its own; `close` aborts a stuck one.
  - The client side speaks no TLS (`Tcp::tls` carries a server certificate and key); a client link
    with `tls` set refuses to connect.
  - `master`'s `with_drain_timeout` and `with_max_inflight` are not carried; the core's drain and
    `Server::max_inflight` cover both.

### 28. UDP

- **Written:** `drain` sends nothing, since UDP has no signal, and keeps receiving so a `cancel`
  still arrives; new calls are answered `unavailable` by the server. A receive error other than a
  connection reset or refusal ends the inbound stream. Ids are mapped per sender address and id.
  `master`'s `with_retries` and `with_retry_backoff` are not carried: no setting in the spine's
  surface names them.

### 29. `ulo dev` endpoint resolution (area D's requests, routed by the coordinator)

- **Written:** the TCP server resolves its endpoint with `EndpointSpec::resolve` in `prepare`; the
  TCP client connects through `resolve_as_written`, so a client whose address equals the socket
  `ulo dev --listen` holds connects to the address rather than taking the inherited listener. UDP
  uses `resolve_as_written` on both sides, so a UDP endpoint at the held TCP address stays an
  address instead of becoming an `Endpoint::Inherited` that its `prepare` refuses.

## Requests for other areas

- **B:** entry 2 (unique ids per inbound stream, one per native correlation); entry 7 (the AMQP
  prefetch has no path from `max_inflight`); entry 24 (Redis shares `Codec::encode_frame`).
- **W:** entry 1's mechanism answers the same question for `IntoReply<Ws>`.
- **S / codegen owner:** `Paths::transport`'s doc says it names `ulo-transport`'s re-export; with
  entry 1 it names any crate whose `__private` supplies `Param`, `controller` and the reply probe.

## Not verified

- Nothing was compiled. Specifically: method probing on the reply probe with a trait type
  parameter (`ViaCallError<M>`) inferring `M` through `V: Answer<M>`; the `TypeId` check for `()`;
  every spawned future being `Send` (the `watch::Ref` is dropped inside a helper for that reason);
  `ciborium` reading a CBOR `null` `d` as `None`; `Option<Box<RawValue>>` reading JSON `null` as
  `None`; `tokio::sync::watch::Sender::new` being in the workspace's tokio.
