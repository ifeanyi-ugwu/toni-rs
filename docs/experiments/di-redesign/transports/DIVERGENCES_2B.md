# Race 2b: divergences for sign-off

Where the code written for race 2b departs from `transports/DESIGN.md`, `DESIGN.md`, the twentieth
response or the fold of `REVIEW_2B.md`, or fills what they leave open, gathered from the eight logs
under `divergences/`: `race2b-S.md` (the spine) and one per area (W WebSocket, R RPC core and the
socket links, B the broker links and the RPC conformance suite, G gRPC and the build step, Q
GraphQL, E the five embedding adapters and the HTTP conformance suite, D the development command).
Also merged: the coordinator's two commits after the race (`50b8439c`, `5d2ad695`), the requests no
commit has applied, the eight choices the fold of `REVIEW_2B.md` made beyond the twentieth
response's text, and what the compile showed. Each item cites its entry: `[S 5]`, `[W 1]`, `[E R1]`
for a request, `[D req 3]`, `[fold]`, `[50b8439c]`. A point several logs cover appears once with
every citation.

Unlike 2a's list, this one is written after a compile. `cargo check --workspace --all-targets`
passes on stable, and on Rust 1.88 for every crate but `ulo-http-salvo` (salvo needs 1.92) and
`ulo-graphql-async-graphql` (async-graphql needs 1.89). A compile checks types; §8 says which
generated code it reached and which it did not. Nothing ran against a socket or a broker.

## How to read it

- **Entries** are numbered U1–U33, continuing 2a's T1–T26. Each gives what the design says, what
  was built, the options, and a recommendation that follows the rules settled in
  `transports/RESPONSE.md`: refuse what `prepare` can see, the `Bound`/`Count` vocabulary, names
  after the operation, one rule without exceptions.
- **Read** lines name the files and lines checked against the claim, as `crate/src/file.rs:line`
  under `crates/`. A claim with no line beside it is the log's and was not re-read here.
- Where two areas disagree with each other, or an area with the design, the entry says so in its
  first sentence.
- §1 holds the decisions, cross-area first and then by area. §2 is what an application writes
  differently. §3 is behaviour filled in that needs no decision. §4 is diagnostics and error text.
  §5 is internal. §6 is what the coordinator applied after the race and the requests still open.
  §7 checks the fold's eight choices against the code. §8 is what the compile showed. §9 closes
  with what to decide before tests and what can follow.

## 1. Decisions for you

### Across areas

### U1. One reply conversion per transport, built three ways [W 1, R 1, R 10, G 10, Q 11, S requests]

- **Design:** §4.1 implements `IntoReply<Ws>` for `()`, `T: Serialize`, `Frame` and a stream of
  `Result<T, E>`; §5.1 the same set for RPC; §2.3 and §6.1 have a gRPC handler's reply convert
  through `IntoReply<T>` and the reply probe; §7 has "the controller's `IntoReply<Http>` for
  `GqlResponse`". The spine left W and R the mechanism, citing E0119.
- **Built:** none of the four can be written as stated. A blanket over a foreign trait
  (`Serialize`, `Stream`, `prost::Message`) puts an uncovered type parameter ahead of the local
  marker, E0210, before coherence is reached; `GqlResponse`, `IntoReply` and `Http` are three
  crates' types. Three answers were built. W and R implement `IntoReply` for their own types only
  (`Frame` and `Reply`; `Reply` and `Data`) and route the handler's value through a crate-local
  probe whose value side is a marker-typed trait `Answer<M>`: three blanket impls under three
  markers (`ViaReply`/`ViaStream`/`ViaSerde` on W, `AsReply`/`AsData`/`AsStream<M>` on R), `M`
  inferred at the call from the one impl the value meets, as `Param<T, M>` already does; `()`
  takes the serde arm and is told apart by `TypeId`. Each attribute points the codegen's
  `Paths::transport` at its own crate's `__private`, so the generated call names that probe with
  the codegen unchanged. G writes, beside each `#[method]` handler, a hidden
  `__ulo_grpc_<name>` method answering a local `Answer` through a six-arm autoref probe; the
  handler value reads the hidden method. Q hand-writes the endpoint controller over
  `ulo_http::__private::HttpHandler`, its two closures building the `Response`. Limits: a type
  both `Serialize` and a stream, or both `Serialize` and a user's own `IntoReply`, is ambiguous at
  the handler (E0283); a stream of bare items is not an RPC answer; `IntoReply<_>` for `()` is
  written nowhere, since it would make every `()` answer ambiguous.
- **Options:** accept the three mechanisms and amend the four design sentences; or one mechanism
  for all three, which the gRPC shape check (U23) rules out for `Answer<M>` and which W and R have
  no hidden method to carry.
- **Recommendation:** accept. W and R share one shape, G's differs for a reason the shape check
  gives, and Q's adds no public type. Amend §4.1, §5.1, §6.1 and §7 to say what a handler may
  return rather than which trait a type implements, and correct the `Paths::transport` doc (U33).
- **Read:** `ulo-ws/src/__private.rs:245-298, 310-363`; `ulo-ws/src/envelope.rs:86`;
  `ulo-ws/src/transport.rs:193`; `ulo-ws-macros/src/message.rs:51`;
  `ulo-rpc/src/__private.rs:129-238`; `ulo-rpc/src/transport.rs:125, 132`;
  `ulo-rpc-macros/src/message.rs:96`; `ulo-grpc/src/__private.rs:166-224`;
  `ulo-grpc-macros/src/method.rs:11-18, 55, 96-97`; `ulo-graphql-http/src/controller.rs:14,
  53-57`.

### U2. A passed deadline on RPC and gRPC renders with no error handler, where HTTP runs them under `timeout_grace` [R 15, G 3]

R and G agree with each other and differ from the rule HTTP settled.

- **Design:** §3.6, as the fourth response's T8 was applied: a route timeout cancels with
  `Deadline` and runs the error handlers with a `Timeout` `CallError` under
  `Server::timeout_grace`, one second at `Default`, rendering the canonical 504 when the grace
  passes first. §5.2 names `deadline-ms` as the execution deadline and §2.6 lists it under
  `Deadline`; §6.2 says `grpc-timeout` "answers DEADLINE_EXCEEDED". Neither names a grace.
- **Built:** both areas drop the pipeline when the clock wins, cancel `Deadline`, and write the
  canonical answer directly: `err` `timeout` on RPC, DEADLINE_EXCEEDED on gRPC. No `Timeout` is
  offered. G's reason is that a caller past its deadline has stopped waiting. A `deadline-ms` that
  does not parse sets none; a `grpc-timeout` off the grammar is logged at `debug` and ignored.
- **Options:** (a) accept both; (b) run the handlers under a `timeout_grace` on `ulo_rpc::Server`
  and `ulo_grpc::Server`, a `Bound`, zero refused in `prepare`, as HTTP does; (c) (b) on RPC alone,
  where the caller reads the envelope.
- **Recommendation:** (b). T8 was settled against the same argument: the grace bounds the handler,
  and an application reshaping every error into its own envelope otherwise sends a raw one on
  timeouts alone. One rule across the four transports. The conformance suites' deadline scenarios
  assert `Deadline` on the execution and are unchanged by it. Decide before tests, since the wire
  answer is what a test pins.
- **Read:** `ulo-rpc/src/dispatch.rs:418-422, 470, 489-491, 529-538`;
  `ulo-grpc/src/dispatch.rs:3-8, 359-366`; `transports/DESIGN.md:494, 846, 1014`.

### U3. What `.at(prefix)` covers: HTTP routes and gateway paths, not RPC patterns or gRPC paths [R 19, G 9, W 6, Q req]

- **Design:** §2.5: `.at(prefix)` is "applied to every route and gateway path of that
  controller"; silent on the join rule and on RPC and gRPC.
- **Built:** RPC patterns and gRPC paths ignore `HandlerInfo::prefix()`. A gRPC method's path is
  its proto's and a client dials it there; refusing the prefix would refuse a controller mixing
  HTTP routes with gRPC methods. A gateway's served path is the connect handler's prefix joined to
  `GatewaySettings::path` by the rule `ulo_http` joins a route with: one `/` between, the prefix's
  trailing slash dropped, so `.at("/graphql")` with path `/` serves `/graphql`, which Q's
  `GraphqlWs` relies on. A prefix on a controller with no HTTP route and no gateway is read by
  nothing and reported by nothing.
- **Options:** accept and amend §2.5; or refuse an unread prefix in `prepare`, which the mixed
  controller rules out.
- **Recommendation:** accept; amend §2.5 to "every HTTP route and gateway path" with the join
  rule written out.
- **Read:** `ulo-grpc/src/server.rs:40`; `ulo-ws/src/gateway.rs:702, 805-816`; no read of
  `prefix()` under `ulo-rpc/src` (grep); `transports/DESIGN.md:271`.

### U4. Three settings refused at `wire()` rather than `prepare`, one of them moved [R 26, Q 19, G 16]

- **Design:** §12 places `Bound::After(Duration::ZERO)` on `RpcClientModule::timeout` and on the
  gateway's `connection_init_timeout` at "startup (`prepare`)"; §5.4 says the same for the client's
  timeout. §6.3's client module parses an endpoint; where a bad URI fails is unstated.
- **Built:** a module has no `prepare`. `RpcClientModule` records a zero timeout through
  `ModuleDef::try_value`, and `wire()` reports it. `connection_init_timeout` moved from the
  gateway's settings to `GraphqlConfig::connection_init_timeout(Bound)` (3 s at `Default`,
  `Unbounded` no clock), because `GatewayConfig::settings()` is a static function; `GraphqlModule`
  binds it as a module-private `InitTimeout` the gateway reads as `Option<Dep<InitTimeout>>`, a
  zero bound through `try_value` with X22's text and reported at `wire()`. `GrpcClientModule`
  binds its parsed endpoint through `try_value`, so a bad URI is a wiring error, and the client
  through `try_singleton`, so the platform roots load and `connect_lazy` runs when the app
  connects, not in `register`.
- **Options:** accept `wire()` for a setting that lives on a module and amend §12's "When"; or
  give modules a `prepare` hook, new core surface for three settings.
- **Recommendation:** accept. The rule's point is one report before any socket, and `wire()`
  comes before `prepare`; a zero on a module is visible there. Amend §12 and §5.4, and record
  `GraphqlConfig::connection_init_timeout` in §7.
- **Read:** `ulo-rpc/src/client_module.rs:57-67`; `ulo-graphql-http/src/module.rs:64-71, 138`;
  `ulo-graphql-ws/src/gateway.rs:74, 170-175`; `ulo-graphql-ws/src/lib.rs:23-24`;
  `ulo-grpc/src/client.rs:43-48, 168`; `transports/DESIGN.md:943` and §12's row.

### U5. The AMQP prefetch is a link setting, where the twentieth response tied it to `max_inflight` [R 7, B 6, S 25]

B differs from the twentieth response; R and B both flag it open.

- **Design:** the twentieth response on R24: "tie the AMQP prefetch default to `max_inflight`
  (`Max(n)` gives a prefetch of n, and otherwise a fixed default such as 64)"; §13 item 15: "the
  AMQP prefetch follows it".
- **Built:** the frozen `Link` gives a link no path to the server's settings: `prepare(&mut self,
  app)` takes the app, `listen` the patterns. B wrote `RabbitMq::prefetch(u16)`, 64 unset, 0
  unlimited as AMQP reads it, applied per consumer before each `basic.consume`; `Server::max_inflight`
  is not read, so the two limits can contradict each other, which the tie existed to prevent.
- **Options:** (a) `Link::listen(patterns, max_inflight)`, a frozen signature changed in seven
  links; (b) a defaulted `fn max_inflight(&mut self, count: Count) {}` on `Link`, called by
  `Server::prepare` beside `Link::prepare`, implemented by the RabbitMQ link, `RabbitMq::prefetch`
  removed; (c) keep the setting.
- **Recommendation:** (b). It restores the settled tie, touches one link's body and the trait,
  and leaves the other six as written. Before tests, since the suite's shed scenario on AMQP
  depends on which limit holds.
- **Read:** `ulo-rpc/src/link.rs:31, 38`; `ulo-rpc-rabbitmq/src/link.rs:45-52, 77-78, 102, 221`;
  `ulo-rpc/src/server.rs:50`; `transports/RESPONSE.md:962-979`; `transports/DESIGN.md` §13 item 15.

### The spine

### U6. X24's probe as built, and `HttpCarried` sealed by convention [S 5, S 6, S 10, fold]

- **Design (fold):** §2.5 and X24: `TransportMetadata { type Transport }`, the probe
  `(&&&MetaProbe::<T, _>::new(&value)).check()` with three arms, a mismatch failing E0308 at the
  value naming both transports, the probe "written once, in `fw-handler-codegen`". X23:
  `HttpCarried` is "a sealed subtrait of `Transport` that `Http` and `Grpc` implement", and
  `m.meta::<PreDispatch<Ws>>()` is E0277.
- **Built:** the arms are `MetaSame` (`&&MetaProbe<T, V>`, `V: TransportMetadata<Transport = T>`),
  `MetaOther<T, U>` (`&MetaProbe`, answering `MetaMismatch<T, V::Transport>`) and `MetaAny`
  (`MetaProbe`); `TransportMetadata: Send + Sync + 'static`, which every metadata value is. The
  generated code binds each `#[meta]` value to a local, probes the local, then records it, so an
  expression with side effects runs once; `emit::metadata` takes the transport's `Paths` for the
  marker, which "written once" did not foresee. `HttpCarried: Transport + __private::Carried`,
  `Carried` public and doc-hidden, implemented by `ulo-grpc` for `Grpc`: the seal holds by the
  `__private` convention and not by the compiler, since a `pub(crate)` seal would keep `ulo-grpc`
  out. The compile reached the matching arm through `#[meta(BodyLimit(16))]` on an HTTP handler and
  no mismatch, so the E0308 arm is unexercised.
- **Options:** accept; or a compiler seal by moving `Grpc`'s impl into `ulo-http` behind a feature,
  which couples the two crates.
- **Recommendation:** accept. `__private` is the workspace's existing boundary for sibling and
  generated code. Amend §12's row to say the seal is doc-hidden, and write a compile-fail test for
  the mismatch.
- **Read:** `ulo/src/__private.rs:102-157`; `ulo-handler-codegen/src/emit.rs:270-289`;
  `ulo-http/src/transport.rs:38-42`; `ulo-http/src/__private.rs:19-21`;
  `ulo-http/src/pre_dispatch.rs:75`; `ulo-http-conformance/src/app.rs:106`;
  `transports/DESIGN.md:288, 479, 1151-1152, 1177-1178`.

### U7. `Serve`'s signatures [S 13, fold]

- **Design (fold):** §3.7 and the plan's row: `Serve::new(listeners, tls, &ServeConfig)`,
  `run(self, connection)`, `drain()`, `close()`, `ServeConfig { handshake_timeout: Option<Duration> }`;
  open whether `Accepted` carries the drain signal.
- **Built:** `new` answers `io::Result<Serve>`, since tokio's `TcpListener::from_std` can fail and
  must run inside the runtime, which a server's `bind` is. `run(&self, ..)`: the core calls a
  server's `serve`, `drain` and `close` on one `&self`, so the server holds one `Serve` across the
  four, and a second `run`, or one after `drain` or `close`, returns at once. `Accepted { io, conn,
  draining }`, `Draining` its own type with `wait` and `is_draining`. `conn` is `ulo_http::ConnInfo`
  with the version HTTP/2 where ALPN settled `h2`, so `ulo-hyper-serve` depends on `ulo-http`.
  `close` aborts through `JoinSet::shutdown` and marks the loop finished, on which `drain` and
  `close` wait; a `drain` or `close` before `run` releases the listeners. `ServeConfig::default()`
  is no handshake clock; a server passes its resolved `handshake_timeout`, which is why the field
  is an `Option<Duration>` and not a `Bound`.
- **Recommendation:** accept; amend §3.7's sketch.
- **Read:** `ulo-hyper-serve/src/serve.rs:22-37, 42, 60, 77, 98, 129, 136`;
  `ulo-hyper-serve/src/listener.rs:49`; `transports/DESIGN.md:540`.

### U8. A module naming one of a transport's seeders is the same declaration; `InputNotSeeded` keeps one `seeder` [S 1, S 2, S 3, S 4]

- **Design:** X19 merges two transports' declarations of one key whose seeder sets are equal;
  §2.10 says a module's declaration and a transport's "with the same `(key, seeder)` is one
  declaration". Silent on a module's `seeded_by::<Tr>()` against a set, and on the report for an
  input with several seeders. `Mounted::handlers_of::<U>()`'s return type is unwritten.
- **Built:** `InputDecl.seeders: Vec<TypeName>`, the declaring transport first. A module's
  `seeded_by::<Tr>()` is the same declaration when `Tr` is among the transport's seeders, and an
  earlier module entry in the graph is widened to the full set, so a handler of every seeder
  passes the input check; a seeder outside the set stays `InputConflict`. `InputNotSeeded.seeder`
  is unchanged and carries the first seeder. `also_seeded_by` with no `input` before it is a no-op:
  the core may not panic and no wiring error exists for it. `handlers_of` answers an owned `Vec`,
  since `Mounted` borrows one transport's handlers; `AppHandle::mounted` fails with `TimerMissing`.
- **Options:** accept; widen `seeder` to a list, a change to a public variant.
- **Recommendation:** accept; §2.10 already says a report names the declaring transport.
- **Read:** `ulo/src/transport/inputs.rs:47`; `ulo/src/graph/mod.rs:169-176`;
  `ulo/src/graph/scopes.rs:387-390`; `ulo/src/graph/wire.rs:367-370, 671-686`;
  `ulo/src/transport/server.rs:89`; `ulo/src/app/handle.rs:123`.

### WebSocket

### U9. `GatewaySettings::port`, with `Port { Http, Own }`, selects which server serves a gateway [W 2]

- **Design:** silent; the spine noted that a gateway on both ports needs a selector. `master`
  decided it by intent, `port = N` on the gateway.
- **Built:** a public field on the `#[non_exhaustive]` settings, default `Http`, a setter, and the
  attribute argument `port = http | own`. The hand-off serves the `Http` gateways on the HTTP
  server's port and on an embedding host declaring `upgrades`; `ulo_ws::Server` serves the `Own`
  gateways, and its `prepare` refuses a server with none. No gateway is served on both, since
  serving every gateway on both ports would expose one on a port its author did not choose, and
  neither server can learn at `prepare` whether the other is bound. Not checked: a `port = own`
  gateway in an application binding no `ulo_ws::Server` is reachable by nothing and nothing
  reports it.
- **Options:** accept; a wiring-time check needs the server's existence, which no module sees.
- **Recommendation:** accept; record in §4.1 and §12, and file the unreported case as a gap.
- **Read:** `ulo-ws/src/gateway.rs:129-130, 150, 214, 241-244`; `ulo-ws/src/server.rs:40,
  248-251`; `ulo-ws/src/handoff.rs:21, 99`.

### U10. Hand-written gateways, the hooks' executions and sessions [W 15, W 16, W 17, S 16]

- **Design:** `Gateway` is the trait a hand-written gateway implements and `GatewayConfig` what
  `#[gateway]` emits; the session factory; `after_init` runs "after `listen()`".
- **Built:** `GatewayConfig: Construct` carries `settings`, `connect_guards`, `session` and
  `mount_gateway`; `Gateway: GatewayConfig` carries `on_message` and defaulted `on_connect`,
  `on_disconnect`, `after_init` and `mount`, because an attributed gateway's hooks are found by
  probing the concrete type, which a generic default cannot do. The instance is resolved once per
  connection in the connection phase's execution and kept; messages reach `on_message` one at a
  time, in order, on a task of their own, one queued or running counting against `max_inflight`; a
  panic in `on_message` closes 1011; a hand-written gateway that also declares `#[message]`
  handlers is refused in `prepare`. `after_init` resolves the gateway in an execution that ends
  before the hook runs, so a long hook holds none across the drain; `on_disconnect` opens a plain
  execution while the app serves and a terminal one once `Execution::open` is refused, and room
  membership is removed after it. A `session_with` factory runs in the connection phase's execution
  with the connect handler's dependencies, so `wire()` checks them; a failure refuses with 1011; a
  gateway with no session answers `Session<T>` as an input never seeded, `KeyName` having no public
  constructor for a `WrongType`.
- **Recommendation:** accept; amend §4.1's two-trait description.
- **Read:** `ulo-ws/src/gateway.rs:60-83, 436-459, 516-559, 721`; `ulo-ws/src/connection.rs:586,
  654, 986-1000`; `ulo-ws-macros/src/gateway.rs:214-237, 272`.

### U11. Broadcasts travel as JSON between processes [W 18, W 19]

- **Design:** §4.3: the adapter's frame is "an encoded envelope", each gateway's codec applied.
- **Built:** `Broadcast::emit` publishes `{"event","data"}` as JSON; each process re-encodes it per
  gateway on delivery, with that gateway's event field and codec, so one publish serves gateways
  of both codecs and either event field. A MessagePack gateway receives the MessagePack form of
  the JSON value, so a byte string arrives as an array of numbers. The Redis adapter's channels are
  `ulo:ws:room:<room>`, `ulo:ws:node:<node>` (a client, published to the process its id names) and
  `ulo:ws:all`, each message the target as JSON, a newline, then the frame; a process subscribes to
  its node's channel and `ulo:ws:all` and pattern-subscribes `ulo:ws:room:*`, since the SPI learns of
  no membership change; a lost subscription reconnects after one second.
- **Recommendation:** accept; record the byte-string cost in §4.3.
- **Read:** `ulo-ws-redis/src/lib.rs:14-17, 51-53, 110-112`.

### U12. `WsModule` is global and carries the hub as module metadata [W 24, W 5, fold]

- **Design (fold):** `WsModule::for_root()` is imported once; it registers the hand-off, binds
  `Rooms` and the adapter, and carries the defaults for the gateways on the HTTP port that
  `ulo_ws::Server` carries for its own.
- **Built:** `register` calls `m.global()`, registers `Handoff::new(defaults, hub)` in `Upgrades`,
  writes the hub as `HubMeta` module metadata that `ulo_ws::Server::prepare` reads, binds `Rooms`
  and the configured adapter under a private key `also_as` `dyn BroadcastAdapter`, exporting both,
  so `Dep<Rooms>` resolves from every module and the standalone server finds the hub whichever
  module imported `WsModule`. An application binding the standalone server without `WsModule` gets
  an in-memory hub of the server's own. The hand-off's `prepare` builds its gateway table with the
  module's defaults and refuses their zeros as the server's does.
- **Recommendation:** accept.
- **Read:** `ulo-ws/src/module.rs:36, 47-91, 105-113`; `ulo-ws/src/handoff.rs:94-99`;
  `ulo-ws/src/gateway.rs:761-793`; `transports/DESIGN.md:726`.

### RPC core, the socket links and the brokers

### U13. What the link contract gained by doc rather than by type [R 2, R 3, R 4, R 5, R 6, B 3]

- **Design:** `Delivery { frame, reply, ack }` and `ReplyPath` are frozen; silent on id
  correlation across callers, a lost caller, the peer address, a reply over the frame limit, and
  which codec a link speaks.
- **Built:** ids are unique per inbound stream, by a doc on `link::Inbound`: TCP and UDP map each
  caller's id to one of their own, the brokers keep a table from the native correlation to a local
  `u64` and the client rewrites each reply's id back. A link that loses a caller delivers `cancel`
  with `reply: None`, read as `Disconnected`; a caller's own `cancel` carries its path and is
  `ClientCancelled` (TCP does the first; no broker loses one caller apart from the others).
  `ReplyPath::peer(self, SocketAddr)` is additive; TCP and UDP attach it to every delivery, an
  event included, for `LinkInfo::peer`, and the brokers leave it unset. A reply send failing
  `FrameTooLarge` is replaced by `err` `internal` under the same id; any other send failure
  cancels the call `Disconnected`. The codec is read off `Capabilities::binary`, CBOR when `true`
  and JSON otherwise, through a crate-private `Codec::of`; a third codec needs a field.
- **Recommendation:** accept; write the four rules into §5.3's SPI text.
- **Read:** `ulo-rpc/src/link.rs:71, 89-97`; `ulo-rpc/src/dispatch.rs:434`;
  `ulo-rpc/src/codec.rs:47`; `ulo-rpc-tcp/src/link.rs:467`; `ulo-rpc-udp/src/link.rs:281`.

### U14. Shape detection reads the return type's spelling for a streamed reply [R 11, R 12]

- **Design:** an `Inbound<T>` parameter is a streamed request, a stream return a streamed reply.
- **Built:** the request side is an autoref probe per parameter, so an alias of `Inbound` counts.
  The reply side is read from the return type as written: an `impl` or `dyn` bound naming `Stream`
  or `TryStream`, or a path segment `BoxStream`/`LocalBoxStream`, anywhere in it. A stream behind
  an alias is declared unary, and a link without streamed shapes then answers it `err` `internal`
  at runtime; an opaque type cannot be named at mount, where `HandlerSpec::shape` is set. Two
  refusals were added: a stream return on `#[event]` is a compile error at the return type, and an
  `#[event]` with an `Inbound<T>` parameter is refused in `prepare`.
- **Options:** accept; an attribute argument naming the shape that overrides the read.
- **Recommendation:** accept; add the override when an alias appears in practice.
- **Read:** `ulo-rpc-macros/src/message.rs:13, 69, 143, 156-185`; `ulo-rpc/src/server.rs:82`.

### U15. Event acknowledgement and mismatched frames [R 13, R 14]

- **Design:** "`req` sent to a streamed-request pattern answers `bad_request`"; an event is
  acknowledged once the handler completes and an unhandled one rejected.
- **Built:** `req` or `evt` to a streamed-request pattern, and `open` to a single-payload or event
  pattern, reach the error handlers through `recover(Some(handler))` as `BadRequest`. A `req` to an
  `#[event]` handler runs it and answers an empty `res`; an `evt` to a `#[message]` handler runs it
  and drops the reply, so `nats req user.created` reaches an event handler (R20). An event is
  acknowledged on `Ok`, rejected without requeue on an error or a passed deadline, and left
  unsettled when shed, refused during the drain or cancelled otherwise, so a broker redelivers it.
  An unhandled event is counted in a server-local counter that only its log line reads.
- **Recommendation:** accept.
- **Read:** `ulo-rpc/src/dispatch.rs:267, 316`.

### U16. TCP and UDP: the frame limit, the client's TLS, and what `master` had [R 27, R 28]

- **Design:** the TCP `max_frame` default is R's open point; §12 refuses `max_frame(0)`.
- **Built:** TCP `max_frame` is 4 MiB, twice HTTP's body limit, capped at the 4-byte prefix's range;
  `max_frame(0)` is refused in `prepare`. A frame that does not decode drops the connection, since
  without its id nothing can be answered. The drain closes the listener and sends `goaway` with
  `try_send`. The TLS handshake has no timeout of its own; `close` aborts a stuck one. The client
  speaks no TLS and a client link with `tls` set refuses to connect. UDP's `drain` sends nothing
  and keeps receiving so a `cancel` still arrives. Not carried from `master`: `with_retries`,
  `with_retry_backoff`, `with_drain_timeout` and `with_max_inflight`; the core's drain and
  `Server::max_inflight` cover the last two.
- **Options:** accept; client TLS and UDP retries as later settings.
- **Recommendation:** accept; file the two as gaps.
- **Read:** `ulo-rpc-tcp/src/link.rs:18-20, 109-115, 157-158`.

### U17. Streamed requests and `cancel` on the brokers: a control lane, an `opened` handshake, best effort [B 1, B 2, B 4, R 24]

- **Design:** §5.2 says what the request and reply lanes carry on NATS, AMQP, MQTT and Kafka and
  is silent on `open`, `in`, `in_end` and `cancel` there; Redis carries the whole frame with
  "replies on a per-client reply channel", silent on how the server learns the channel; §5.4:
  dropping a stream or a pending request sends `cancel`.
- **Built:** `open` travels on the request lane as an empty body with a reserved header
  `ulo-t: open` and the reply address; the server holds the call, answers a header-only
  `ulo-t: opened` on the reply lane, then delivers `open`. `in`, `in_end` and `cancel` travel on
  one control lane per link that every instance reads outside the competing group (the subject
  `ulo.rpc.control`, a fanout exchange of that name with an auto-delete queue per instance, the
  topic `ulo/rpc/control` outside `$share`, the topic `ulo.rpc.control` assigned in full from its
  end), each message carrying the call's native correlation, each instance delivering only those
  naming a call it holds; the client queues a streamed request's control frames until `opened`.
  A `cancel` of a unary or server-streamed call is published at once; one that overtakes its `req`
  finds no held call, is dropped, and the call runs to its end, the race the old links had.
  Redis: a `req` or `open` carries `ulo-reply` in `h` naming the caller's channel, stripped before
  a handler reads the headers; each caller offsets its ids by a random `u64` base; `in`, `in_end`
  and `cancel` travel on `ulo:rpc:control`; no handshake, since one publisher and one Pub/Sub
  connection deliver in order. Costs: every instance receives every control message, and a client
  stream waits one round trip before its first item.
- **Options:** accept; a handshake on `req` too, closing the race at a round trip per call;
  per-instance request subjects.
- **Recommendation:** accept and write it into §5.2 and the link table, the race included; the
  suite's cancel scenario accepts either outcome.
- **Read:** `ulo-rpc-nats/src/link.rs:18-32, 247-248, 432-435, 535-536`;
  `ulo-rpc-rabbitmq/src/link.rs:25-39, 478-481, 592-593`; `ulo-rpc-mqtt/src/link.rs:23-33`;
  `ulo-rpc-kafka/src/link.rs:360-376`; `ulo-rpc-redis/src/link.rs:20-25, 101-103, 377, 388-396,
  414-422`; `transports/DESIGN.md:850, 915`.

### U18. A RabbitMQ queue outlives its server, so a declared `miss_signal` answers `Timeout` for a pattern served once [B 7]

- **Design:** §5.2: `miss_signal` declares whether a link signals that nothing listens; the suite
  asserts `Unavailable` where it is `true`; AMQP's signal is `basic.return` on a `mandatory`
  publish.
- **Built:** each pattern's queue is declared with default options, neither auto-delete nor
  exclusive. A request sent while no instance consumes waits in the queue for the next one, so the
  caller sees its own `Timeout` though the link declares `miss_signal`; `basic.return` fires only
  for a pattern no server ever declared. An auto-delete queue would lose the requests it holds
  when its last consumer cancels, which the drain does.
- **Options:** accept and state when a miss on AMQP is `Timeout`; auto-delete queues; a queue TTL.
- **Recommendation:** accept and state it in §5.3's table beside Kafka's conditional miss; the
  suite's miss scenario uses a pattern never declared.
- **Read:** `ulo-rpc-rabbitmq/src/link.rs:230`.

### U19. Links connect once at `bind`; an unreachable broker fails `bind` [B 14]

- **Design:** silent. `master`'s NATS adapter carried `retry_on_initial_connect`.
- **Built:** every broker link connects in `listen` or `connect` without retrying, failing as
  `StartupError::Bind`. After `bind`, NATS and lapin recover by themselves and the Redis, MQTT and
  Kafka links reconnect in their read loops.
- **Recommendation:** accept; one report at startup, and a retry window is a `Bound` to add if a
  deployment needs one.
- **Read:** `ulo-rpc-mqtt/src/link.rs:160-175`; B 14 for the other four.

### U20. Kafka's declared frame limit, delivery timeout and control consumer; the group default on three brokers [B 10, B 12, B 8]

- **Design:** Kafka's "size limit from broker configuration"; NATS "maximum payload read from the
  server's `INFO`", MQTT "from CONNACK"; §13 item 13: the group defaults to the root module's full
  type path on NATS and MQTT.
- **Built:** Kafka declares `max_frame: Some(1_000_000)`, the producer's `message.max.bytes`
  written out, a lower broker answering `MSG_SIZE_TOO_LARGE`, mapped to `FrameTooLarge` too;
  `message.timeout.ms` is 30 s rather than five minutes; the control consumer is assigned every
  partition of `ulo.rpc.control` from its end under a `group.id` of its own that commits nothing,
  partitions read with `fetch_metadata` on a blocking thread; `topic_partitions` and
  `replication_factor`, 1 each, name the creation knobs. NATS and MQTT answer `max_frame: None`
  until a connection has read the value. The group is `format!("{:#}", app.root().name())` read in
  `prepare`, sanitised per broker (whitespace, `*` and `>` on NATS; whitespace, `/`, `+` and `#` on
  MQTT), a `.group(..)` the broker would refuse failing `prepare` by name; Kafka's consumer group
  defaults to the same path, unsanitised, where `master` had a fixed `ulo-rpc-server`. The
  caller-supplied ordering key §5.3 names has no spelling on `RpcClient`.
- **Recommendation:** accept; record the three in §5.3 and file the ordering key as a gap.
- **Read:** `ulo-rpc-kafka/src/link.rs:45-51, 110-117, 146, 345, 360-376`;
  `ulo-rpc-nats/src/link.rs:99`; `ulo-rpc-mqtt/src/link.rs:52, 709-710`.

### gRPC and the build step

### U21. gRPC fires `ClientCancelled` for every abandonment hyper reports [G 4]

G differs from §2.6.

- **Design:** §2.6: `Disconnected` for a closed connection or an h2 `RST_STREAM`;
  `ClientCancelled` for "a gRPC `RST_STREAM(CANCEL)`".
- **Built:** `ClientCancelled` when hyper drops the call's future before it answered and when it
  drops the reply body before its end. hyper reports neither the reset's error code nor whether
  the connection closed; it drops the future or the body in both cases.
- **Options:** accept and amend §2.6 for gRPC; `Disconnected` for both, which misnames the common
  case, since a gRPC caller abandoning a call resets with CANCEL.
- **Recommendation:** accept; amend §2.6.
- **Read:** `ulo-grpc/src/dispatch.rs:423-430, 548, 638`; `transports/DESIGN.md:301-302`.

### U22. A `tonic::Status` returned as an error reaches the wire as it stands; `CallError::with_grpc_code` and `grpc_code()` [G 8, G 1]

- **Design:** §2.4 maps every error by kind; decision 2 names `CallError::grpc_code(..)` as the
  per-error override of `Conflict`'s ABORTED; silent on a handler returning a `Status`. The spine
  left `grpc_code` unowned, `CallError` being `ulo-transport`'s.
- **Built:** `render` downcasts what the error handlers left to `tonic::Status` first and sends it
  unchanged, the one way to name a code outside the eleven kinds without a `CallError`; every
  other error goes through `from_boxed`, which would answer `Internal` for a `Status`. On
  `CallError`, written by G in `ulo-transport` under the coordinator's exception: a private
  `grpc_code: Option<i32>`, `None` from `new`, copied by `summary()`; the builder
  `with_grpc_code(i32)` and the accessor `grpc_code()`, after `with_details` and `details()`;
  `to_status` uses the code when it lies in 1 to 16 and the kind's code otherwise. `i32` because
  `ulo-transport` depends on no gRPC crate.
- **Recommendation:** accept both; document the `Status` passthrough in §6.2 beside
  `with_grpc_code`.
- **Read:** `ulo-grpc/src/status.rs:50-60, 68-71`; `ulo-transport/src/error.rs:90, 96, 185-194`.

### U23. The dispatcher on tonic's codec layer, a fixed 4 MiB message limit, and the answers no error handler sees [G 2, G 18, G 5, G 7, G 6, G 11, G 12]

- **Design:** §6.1 dispatches through `tonic::server::Grpc` and the four shape-service traits,
  the plan leaving G to confirm it suffices; the marker checks are "trait bounds" with the
  mechanism unwritten; §6.2: over the in-flight limit UNAVAILABLE; silent on message size,
  compression and where health and reflection sit in the stage.
- **Built:** `Grpc::unary` and its siblings decode before the service runs, where the guards would
  then run after a decode against §2.2, and answer an unencoded `Response<M>` where the spine's
  `Grpc::Reply` is the encoded `http::Response` interceptors read; the dispatcher uses the two
  pieces `Grpc` composes, `Streaming::new_request` in the extractors and `EncodeBody::new_server`
  for replies. The extractors decode under tonic's own 4 MiB limit, a larger message failing as
  `ExtractError::TooLarge` (INVALID_ARGUMENT); the `Server` has no setting. The marker checks are
  const generics at the concrete site: one `ParamProbe` per parameter and the reply probe each take
  `{ streams_request(<Marker as Method>::SHAPE) }` or `streams_reply(..)`, a mismatch leaving
  `ParamCheck::matches` or `Checked::checked` uncallable, E0599 naming both types; a handler
  reading no message is accepted with its request unread. `GrpcHandler` lost its `shape` field,
  which nothing read. A `grpc-encoding` other than `identity` answers UNIMPLEMENTED with
  `grpc-accept-encoding: identity` before dispatch; over `max_inflight` or `max_per_connection`,
  and during the drain, UNAVAILABLE before an execution opens, no `Retry-After`; health and
  reflection are routed after the unscoped entries with no scoped entries and no error handlers,
  since no handler matched.
- **Options:** accept; add `message_limit: u64` on `ulo_grpc::Server`, the vocabulary's `*_limit`.
- **Recommendation:** accept, and add `message_limit` as a follow-up; amend §6.1's mechanism text.
- **Read:** `ulo-grpc/src/dispatch.rs:3-8, 70-72, 345`; `ulo-grpc/src/extract.rs:18`;
  `ulo-grpc/src/server.rs:107-113`; `ulo-grpc/src/__private.rs:41-47, 66-140`;
  `ulo-grpc-macros/src/method.rs:96-97`.

### GraphQL

### U24. The engine lives in a global, exporting module; the context resolves in that module; `GraphqlModule` mounts the gateway and depends on `ulo-ws` [Q 1, Q 2, Q 3, Q 18]

Q differs from §7's example and from §1's crate layout.

- **Design:** §7's example binds the engine in `ApiModule`, which imports
  `GraphqlModule::for_root(..)`; the adapter resolves the context with `exec.get::<C>()`; §1 has
  `ulo-graphql-http` meet the core through `ulo-http` alone, with the gateway in
  `ulo-graphql-ws`.
- **Built:** `GraphqlModule` registers the endpoint, and with `subscriptions(path)` the
  `GraphqlWs<Q>` gateway `.at(path)`, as its own controllers reading `Dep<dyn Engine, Q>`. By
  DESIGN §8.2 they see their own bindings, their imports' exports and the globals' exports, none
  of which is the importer, so the example as written fails `wire()` with the engine missing. The
  crate docs rewrite it with `global` and `exports = [dyn Engine]`. Each adapter keeps its own
  `ModuleRef` and resolves `module.with_execution(&exec).get::<C>()` per call, built once in the
  call's execution; neither declares `Dep<C>`, so an unbound context fails each call rather than
  `wire()`, since declaring it would make the engine per-execution under an execution-scoped
  context. `ulo-graphql-http` depends on `ulo-graphql-ws`, so on `ulo-ws`; importing `WsModule`
  from `GraphqlModule` would make a second `WsModule` identity beside the application's, so the
  application still imports it once. Open: whether async-graphql's `dep::<T>(ctx)` should read
  with the engine module's visibility; both adapters keep the design's reading so one name means
  one thing.
- **Options:** accept and rewrite §7's example and §1's layout; or new core surface for a module
  to register a controller into its importer.
- **Recommendation:** accept; the alternative is core surface for one crate. Rewrite §7 and §1,
  and state the `Dep<C>` trade in §7.
- **Read:** `ulo-graphql-http/src/lib.rs:8-23`; `ulo-graphql-http/src/module.rs:55-73`;
  `ulo-graphql-http/Cargo.toml:16`; `ulo-graphql-async-graphql/src/lib.rs:32-39, 111-119`;
  `ulo-graphql-juniper/src/lib.rs:217-219`.

### U25. An absent `Accept` negotiates `application/graphql-response+json` [Q 12, Q 13]

Q differs from §7.

- **Design:** §7: `application/graphql-response+json` "when accepted and `application/json`
  otherwise", which makes an absent `Accept` legacy JSON.
- **Built:** no `Accept`, or only empty ones, negotiates `application/graphql-response+json`.
  GraphQL-over-HTTP reads it so from its 2025-01-01 watershed, and §0.3 lets the specification
  decide. Qualities are read per RFC 9110, a type's quality that of the most specific matching
  range; the response is `application/graphql-response+json` when its quality is nonzero and at
  least `application/json`'s, `application/json` otherwise, and no 406 is sent. The consequence:
  a client sending no `Accept` receives 400 for a document that does not parse.
- **Recommendation:** accept; amend §7.
- **Read:** `ulo-graphql-http/src/controller.rs:129-150`; `transports/DESIGN.md:1078`.

### U26. graphql-transport-ws operations bypass `dispatch`; a context that cannot be built is a request error [Q 22, Q 4]

- **Design:** R37 makes the gateway a hand-written `Gateway`; silent on enhancers per `subscribe`
  and on a context that fails to build.
- **Built:** each `subscribe` runs in a `tokio::spawn`ed task holding its `Execution`, opened on
  the connection; the connect guards run once per connection, and no `Ws` guard, interceptor or
  error handler runs per operation, the hand-written gateway having no handler for them to wrap.
  A context lookup that fails is logged at `error` and the call answers
  `GqlResponse::request_error(["the request's context could not be built"])`, 400 under the new
  media type, since `Engine::execute` has no error channel and `Outcome` has two values. Q
  proposes `Outcome::Failed`, rendered 500; `Outcome` is not `#[non_exhaustive]` and is matched
  only in `ulo-graphql-http`.
- **Options:** accept both; add `Outcome::Failed`; route each `subscribe` through a synthetic
  handler so the `Ws` tiers run.
- **Recommendation:** add `Outcome::Failed` before the first GraphQL test, since a server fault
  answered 400 tells a client to fix its document. Accept the per-operation gap for 2b: the
  connect guards are the gateway's authorization point, and per-operation enhancers are a gap to
  file.
- **Read:** `ulo-graphql-ws/src/gateway.rs:201-215, 232, 253`;
  `ulo-graphql-async-graphql/src/lib.rs:115-119`; `ulo-graphql-juniper/src/lib.rs:217-219`;
  `ulo-graphql/src/lib.rs:108-114`.

### The five embedding adapters and the HTTP conformance suite

### U27. The three `embed.rs` additions as built, and the one E request left [E R1, E R2, E R3, E 12, 50b8439c]

- **Design:** §3.8: salvo, actix and rocket strip `.nested_at` themselves, and a path outside the
  prefix is answered as the app's 404 with `Routing::NotFound`, logged at `warn` once; the tenth
  response: an adapter's built-in values are `forward` registrations inside its `Embedded::new()`;
  the twentieth response's answer 3: rocket buffers under the embedding's own `body_limit`; §3.8:
  the axum adapter reads the host's `NestedPath` and logs a mismatch against `.nested_at`.
- **Built (by the coordinator, after the race):** `Embed::STRIPS_PREFIX: bool = true`; when
  `false`, `Service::respond` strips the normalized prefix from the path keeping the query, and a
  path outside it is marked with a crate-private `OutsidePrefix` that routing answers as
  `Miss::NotFound` before the router runs, so an in-app route cannot match by accident, logged at
  `warn` once per embedding. `Embed::builtin_forwards(Embedded<Self>) -> Embedded<Self>`, called by
  `Embedded::new`: salvo, poem, actix and rocket register `OriginalPath` there, and axum keeps its
  layer. `Handle::body_limit()` reads the config; rocket's handler buffers under it. axum and poem
  declare `STRIPS_PREFIX: true`, the other three `false`, so the rule holds by construction. Not
  written: the `NestedPath` mismatch check, since `HostLayer` is a unit struct with `S::Future`
  (both frozen) and `ulo-http` exposes no prefix; E's optional request is an `embed::HostPrefix`
  extension a tower layer inserts and `Service` compares with the normalized prefix once.
- **Recommendation:** accept the three as built; defer `HostPrefix` until a wrong `.nested_at` is
  seen in practice.
- **Read:** `ulo-http/src/embed.rs:85-96, 273-274, 661-669, 753, 845-880`;
  `ulo-http/src/service.rs:207-209`; `ulo-http-axum/src/lib.rs:41`;
  `ulo-http-salvo/src/lib.rs:39-55`; `ulo-http-poem/src/lib.rs:37-45`;
  `ulo-http-actix/src/lib.rs:47-60`; `ulo-http-rocket/src/lib.rs:46-61`;
  `ulo-http-rocket/src/handler.rs:69`; `ulo-http-axum/src/layer.rs:22, 54, 61`.

### U28. rocket reads the body lazily under `body_limit + 1`, reports the default limit, and empties `shutdown.signals` [E 5, E 6, E 4]

- **Design:** `request_body: Buffered(body_limit)`; `Miss::Forward` hands back "the request's
  original `Data`, unread"; `shutdown.ctrlc = false` and `shutdown.grace` before `ignite()`.
- **Built:** the app's body asks for the read on its first poll; the handler then reads
  `body_limit + 1` bytes of `Data` while awaiting the answer and hands them over as one frame. A
  body the app never polls leaves `Data` unread for a forwarded miss. Over the limit the app
  refuses with the same 413 as everywhere; under a route whose own limit is higher an error frame
  follows, so the app never takes the truncated body for the whole one. `Embed::limits()` is an
  associated function with no instance to read, so it reports `Buffered(HttpConfig::default()
  .body_limit)`, 2 MiB. `run` also sets `shutdown.signals = []`, since rocket otherwise handles
  SIGTERM on Unix; an ignite failure is installed as a host future failing at once, and errors
  cross as their `Display`, which marks rocket's error handled. `Rocket<Ignite>::shutdown()`
  exists, so the handle is taken before `launch()`.
- **Recommendation:** accept.
- **Read:** `ulo-http-rocket/src/lib.rs:57`; `ulo-http-rocket/src/handler.rs:24, 46, 69, 76`;
  `ulo-http-rocket/src/run.rs:13-18, 30-31, 54`.

### U29. actix: no response pump; `ActixScope` is a `ServiceFactory` [E 10, E 11]

- **Design:** "on actix the drop is the worker-local pump's"; the spine's `pump.rs` doc pumps the
  response too; `App::service(scope("/api", embedded))` or `default_service`.
- **Built:** only the request payload is pumped. The response is a `MessageBody` over the app's
  `Send` body, polled on the worker, `BodySize::Sized` where the size is exact, dropped at the next
  failed write, which `AtNextWrite` declares; the pump existed on `master` to carry actix's `!Send`
  body into a `Send` one. `ActixScope` implements `ServiceFactory<ServiceRequest>`, so
  `App::default_service(scope("", &embedded))` mounts it as the fallback; `ActixService` is
  exported; of the app's response extensions only `Routing` is copied into actix's store.
- **Recommendation:** accept; amend §3.8.
- **Read:** `ulo-http-actix/src/pump.rs:1-4`.

### U30. The suite's limits: the reference host by `TypeId`, no pre-`listen()` 503, one direction of `host_extensions` [E 15, E 17, E 21, E 14]

- **Design:** the lifecycle scenario covers "the 503 before `listen()` and after `close`"; the
  `Host` trait and `Mode` are the spine's.
- **Built:** where a backend cannot do what a host does, a scenario checks
  `TypeId::of::<H>() == TypeId::of::<HyperHost>()` and runs the half that applies, since the frozen
  `Host` trait cannot say it. `Host::start` returns once the app listens and every `run` serves
  the host only after `listen()`, so the 503 before `listen()` is unreachable through `Host`;
  after `close` the scenario accepts no listener or a 503 with `Connection: close`.
  `present_and_absent` returns at once on a host declaring `host_extensions: false`, because a
  `Host<HostValue>` read with no copy is refused at `prepare` and the suite's app cannot run there
  without the copy `start` registers. Added to the suite's surface: `app_for(EmbedLimits)` (the
  app without its upgrade handler where `upgrades` is `false`), `ROUTING_HEADER`,
  `routing_label`, `HostValue`, `HOST_VALUE_HEADER` and `ORIGIN`, the contract through which a
  host writes `Routing` into a header and copies `HostValue` into its own store.
- **Recommendation:** accept; a `Host` method for a host started before `listen()` is a
  follow-up.
- **Read:** `ulo-http-conformance/src/wire.rs:55`; `ulo-http-conformance/src/lib.rs:54, 58,
  73-76, 109`; `ulo-http-conformance/src/app.rs:25-33`.

### The development command

### U31. `ulo dev`: build then swap, a two-second grace, what `--listen` accepts, Windows [D 7, D 8, D 9, D 11, D 15, D 16, D 1, D 13, D 14]

- **Design:** §9: "rebuilds (`cargo build`)", signals the old child, starts the new one on the
  same sockets; the socket is passed "with the address text as the socket's name"; `ULO_DEV=1` is
  set by the command; on Windows the command restarts without holding sockets.
- **Built:** each change runs `cargo build --message-format=json-render-diagnostics` with the
  mirrored flags while the old child serves; only a build that succeeds stops it and starts the
  binary cargo's artifact messages name, chosen as `cargo run` chooses (`--bin`/`--example`, the
  only binary, `default-run` from `cargo metadata`, else an error naming `--bin`); a failed build
  leaves the application running; a change during a build builds again before swapping, so a
  binary behind the source never starts. `STOP_GRACE` is 2 s for a restart and for Ctrl-C, carried
  from `master`, where an application's drain window defaults to 10 s: an open WebSocket or SSE
  client holds a restart 2 s and is then cut, and `DevArgs` carries no knob. `--listen` takes an IP
  socket address, optionally `tcp://`; a bare port (`master` read `8080` as `127.0.0.1:8080`),
  `udp://` and a host name are each refused with their own message, since the held socket goes to
  the endpoint written as the same address. Off Unix `--listen` is accepted with a warning and the
  application binds the address itself, where `master` refused the flag. The socket's name is the
  address with `%` as `%25` and `:` as `%3A`, because `LISTEN_FDNAMES` separates names with `:`;
  the trampoline runs only when a socket is held; every child gets `ULO_DEV=1` with `LISTEN_PID`
  removed, and `LISTEN_FDS`/`LISTEN_FDNAMES` only with a held socket. `watchexec` is kept over bare
  `notify`, its supervisor running the child and the build on `tokio::process` outside the job.
- **Options:** accept; a `--stop-grace` flag; refuse `--listen` off Unix as `master` did.
- **Recommendation:** accept; `--stop-grace` as a follow-up; the documentation states the four.
- **Read:** `ulo-cli/src/commands/dev.rs:41-44, 125-126, 261-267, 283, 299-305, 445-483, 523,
  543, 574-593`; `ulo-cli/src/main.rs:22, 26`; `ulo-cli/src/commands/exec.rs:1, 22, 42`;
  `ulo-cli/Cargo.toml:17-42`.

### U32. `EndpointSpec::resolve_as_written` is new; the TCP client and UDP use it [D 6, D 3, R 29, D req 1, D req 2]

- **Design:** §9: an `Endpoint::Addr` "whose text equals an inherited socket's name" adopts it
  under `ULO_DEV`; "without the variable, or with no name matching, the address binds as written";
  the plan froze `resolve`'s signature.
- **Built:** `resolve` calls `resolve_as_written` then, under `ULO_DEV=1`, looks the parsed address
  up in `Activation::get()`, `PidMismatch` and `Unsupported` logged at `debug` and any other error
  at `warn`, falling back to the address in every case. `resolve_as_written` is the old body, made
  public: two callers must not take the lookup and `resolve` has no parameter to say so. The TCP
  client connects through it, so a client in a `ulo dev` child whose address equals the held
  socket connects rather than adopting; UDP uses it on both sides, so a UDP endpoint at the held
  TCP address stays an address instead of an `Endpoint::Inherited` its `prepare` refuses. D's two
  requests to R are applied. The address is compared as parsed, so three spellings of one address
  match.
- **Recommendation:** accept; the doc on `Activation::get` still names `Endpoint::Inherited` as
  its only caller (U33).
- **Read:** `ulo-net/src/endpoint.rs:96-160`; `ulo-rpc-tcp/src/link.rs:133, 197`;
  `ulo-rpc-udp/src/link.rs:68`.

### Requests no commit applied

### U33. Four requests still open, and one retirement [Q req, D req 3, R req, W retired, E 12]

- **`MethodNotAllowed::new` is `pub(crate)`.** Q's GET-mutation 405 is built in the controller with
  `Allow` and a request-error body and offered to no error handler, where the router's 405 is.
  Make the constructor public and have the controller offer it: one rule for a 405. Before tests.
- **`Activation::get`'s doc** says to call it only for an `Endpoint::Inherited`; `EndpointSpec::resolve`
  now calls it for an `Endpoint::Addr` under `ULO_DEV=1` and reads `PidMismatch` as no sockets.
  Doc only.
- **`Paths::transport`'s doc** says it names `ulo-transport` as the transport crate re-exports it;
  with U1 it names any crate whose `__private` supplies `Param`, `controller` and the reply probe.
  Doc only.
- **`ulo-ws-tungstenite`** is replaced by `ulo_ws::Server`; its directory still sits in the
  workspace `exclude`. Delete the directory and the entry. **`watchexec-events`** is in the
  workspace manifest and no member uses it, `ulo-cli` having dropped the seeded startup event.
  Drop it.
- **E's `HostPrefix`** is optional and deferred (U27).
- **Read:** `ulo-http/src/miss.rs:53`; `ulo-graphql-http/src/controller.rs:75, 98, 252-255`;
  `ulo-net/src/activation.rs:51-58`; `ulo-handler-codegen/src/paths.rs:13-14`; `Cargo.toml:44-56,
  133`; `ulo-cli/Cargo.toml:17-42`.

## 2. What you will write differently

- **WebSocket** [S 16-20, W 2, W 23]: `#[ulo_ws::gateway(path = "/..", namespace, event, codec,
  subprotocols, session | session_with, port = http | own, ..)]` on the `#[routes]` impl and
  `#[ulo_ws::message("event")]` on each handler; a hand-written gateway implements `GatewayConfig`
  (`settings`, `connect_guards`, `session`, `mount_gateway`) and `Gateway` (`on_message`, with
  `on_connect`, `on_disconnect`, `after_init` and `mount` defaulted). `GatewaySettings::at(path)`
  with a setter per field, `message_limit` an `Option<u64>` deferring to the server or `WsModule`.
  A handler returns a `Frame`, a `Reply`, any `Serialize` value, `()` or a stream of
  `Result<T: Serialize, E: Into<CallError>>` (U1); `WsCx` is an ordinary parameter and
  `Valid<Payload<T>>` validates. `ConnectRefused::code(code, reason)?` or `::kind(kind, reason)`;
  `ConnId { node, seq }`; `Rooms::to_*` answer a `Broadcast` whose `emit` is
  `Result<(), BroadcastError>`. `WsModule::for_root().broadcast(adapter)` plus `message_limit`,
  `max_connections`, `max_inflight`, `max_outbound`, `ping_interval` and `pong_timeout` for the
  same-port defaults; `ulo_ws::Server` for `port = own` gateways. Read:
  `ulo-ws/src/gateway.rs:129-150, 214`; `ulo-ws/src/module.rs:47-91`;
  `ulo-ws/src/transport.rs:119, 214`; `ulo-ws-macros/src/gateway.rs:226`.
- **RPC** [S 21-29, R 1, R 9, R 25, R 26]: `#[ulo_rpc::message("p")]` and `#[ulo_rpc::event("p")]`
  on a `#[routes]` impl; a handler returns a `Reply`, a `Data`, any `Serialize` value, `()` or a
  stream of `Result<U: Serialize, E>` with `E: Into<CallError>` or `E = BoxError` (U1); a bare
  `Bytes` parameter is the payload as `Payload<Bytes>` is; an `Inbound<T>` parameter is a streamed
  request. `Reply` carries `Data`, not `Frame`. Links are `Tcp::new`, `Udp::new`, `Nats::url`,
  `Redis::url`, `RabbitMq::url`, `Mqtt::url`, `Kafka::brokers`, with `group` on NATS, MQTT and
  Kafka, `reply_topic`, `topic_partitions` and `replication_factor` on Kafka, `prefetch` on RabbitMQ
  (U5), `max_frame` on TCP, `tls` on TCP (server side only); `Nats::url` takes a comma-separated
  list and `IntoNatsServers` is gone. `RpcClientModule::for_root(link).timeout(Bound)`; the client
  offers `request`, `emit`, `stream`, `send_stream` and `duplex` as `IntoFuture` builders, `.within
  (&exec)` stamping `deadline-ms`. The SPI lives in `ulo_rpc::link`, re-exported at the root but
  for `Inbound`, which at the root is the extractor. Read: `ulo-rpc/src/lib.rs:31, 41`;
  `ulo-rpc/src/extract.rs:41`; `ulo-rpc/src/client_module.rs:50-73`; `ulo-rpc-nats/src/link.rs:59,
  71`; `ulo-rpc-kafka/src/link.rs:76-117`; `ulo-rpc-rabbitmq/src/link.rs:64, 77`;
  `ulo-rpc-tcp/src/link.rs:109`.
- **gRPC** [S 30-35, G 10, G 13, G 16, G 20]: `build.rs` is
  `ulo_build::configure().file_descriptor_set_path(..).build_client(..).compile(&protos,
  &includes)`; `include_proto!("pkg")` includes `$OUT_DIR/<package>.rs`, which carries a
  `<service_snake>` module of `Method` markers beside `<service_snake>_client` and, with clients
  on, a `GrpcClient` impl. `#[ulo_grpc::method(pb::user_service::GetUser)]` on each handler of a
  `#[routes]` impl; parameters `Message<T>`, `Request<T>`, `Streaming<T>`, `&GrpcCx` and
  extractors; the reply a prost message, `()`, `Response<T>` with `.metadata(k, v)`, a stream of
  `Result<M, E: Into<CallError>>`, or a `Result` of either; a `tonic::Status` returned as the error
  travels as it stands (U22); `.with_grpc_code(n)` on a `CallError`. `ulo_grpc::Server::new(..)`
  adds `handshake_timeout(Bound)`, `file_descriptor_set(&'static [u8])` and `reflection(bool)`,
  on in debug builds. `GrpcHealth::module()` for an injectable reporter. `GrpcClientModule::<C>::
  for_root(GrpcEndpoint::new(uri).tls(ClientTls::system_roots() | ca_pem(..)))`, `https` without
  `.tls` trusting the platform roots. The user's crate names `tonic`, `tonic-prost` and `prost`.
  Read: `ulo-build/src/lib.rs:41-83`; `ulo-grpc/src/lib.rs:57`; `ulo-grpc/src/transport.rs:152`;
  `ulo-grpc/src/server.rs:85-86, 128-129`; `ulo-grpc/src/health.rs:27`;
  `ulo-grpc/src/client.rs:19, 87-98`.
- **GraphQL** [S 36-39, Q 1, Q 5, Q 19, Q 20]: the engine's module is `global` and
  `exports = [dyn Engine]` (U24); `GraphqlConfig::at(path).subscriptions(path).playground(bool)
  .connection_init_timeout(Bound).engine::<Q>()` and `GraphqlModule::for_root(config)` with `Q = ()`
  unqualified; a resolver reads `ctx.data::<Dep<GqlContext>>()` on async-graphql; juniper's engine
  needs the `schema-language` feature for `sdl`. Read: `ulo-graphql-http/src/module.rs:98-144`;
  `ulo-graphql-async-graphql/src/lib.rs:139`; `ulo-graphql-juniper/Cargo.toml:17`.
- **Embedding** [S 40-42, E R2, E 11, E 14]: `ulo_http_salvo::handler(&handle)`,
  `ulo_http_poem::endpoint(&handle)`, `ulo_http_actix::scope(path, &handle)` (also
  `App::default_service(scope("", &handle))`), `ulo_http_rocket::routes(&handle)`, and axum's
  `nest_service("/api", HostLayer.layer(embedded.service()))`; `OriginalPath` arrives without a
  `forward` on every host. `run(app, &handle, host, signal)` answers `Result<Shutdown, BoxError>`.
  The suite is `ulo_http_conformance::{Host, Mode, PREFIX, app, app_for, HyperHost}` and its macro.
  Read: `ulo-http-axum/src/layer.rs:22`; `ulo-http-conformance/src/lib.rs:54, 73-76, 120`.
- **`ulo dev`** [D 15, D 16, D 7]: `ulo dev --listen 0.0.0.0:8080` (or `tcp://`); no bare port, no
  `udp://`, no host name; the application writes `Server::new("0.0.0.0:8080")` unchanged and
  adopts the socket under `ULO_DEV=1`. Read: `ulo-cli/src/commands/dev.rs:261-267`.
- **Core and `ulo-transport`** [S 7, S 2, G 1]: a server's `prepare` collects failures in
  `ulo_transport::prepare::Failures` (`push`, `push_error`, `extend(zero_bound(..))`,
  `extend(zero_count(..))`, `into_result`), a `Failure` built with `plain`, `naming` or
  `from_error`; `Inputs::input::<T>().also_seeded_by::<U>()`; `CallError::with_grpc_code(i32)`.
  Read: `ulo-transport/src/prepare.rs:29-58, 98-118, 193, 206`; `ulo/src/transport/inputs.rs:47`.

## 3. Behaviour that differs or was filled in

### Core, `ulo-transport` and `ulo-http`

- X22's `Failure` is opaque, built with `plain`, `naming` or `from_error`; `push_error` and
  `from_error` keep a `PrepareError`'s names, so a link's or an upgrade handler's typed failure
  enters the naming pass; the two zero checks answer `Option<Failure>`; `into_error` on no failure
  writes "prepare failed" [S 7]. `span::call` declares `ws.event` up front, since W cannot edit
  `span.rs` [S 8]. Read: `ulo-transport/src/prepare.rs:98-118, 193, 206`;
  `ulo-transport/src/span.rs:30`.
- `UpgradeHandler::bound` returns nothing; work that waits, a gateway's `AfterInit`, is the
  handler's to spawn, since `AfterInit` runs "after `listen()`", which no core hook marks. Every
  registered handler, deduplicated by `Arc` identity, runs `prepare` inside the shared
  `prepare_app`, on a backend and on an embedding alike; `bound` runs after the backend binds and
  in `Embedded::bind`; `drain` and `close` are joined with the backend's or the host's [S 9].
  Read: `ulo-http/src/upgrade.rs:25-57`; `ulo-http/src/server.rs:302, 361-373, 426, 437, 441`;
  `ulo-http/src/embed.rs:58, 562, 582`.
- `ulo_http::stage::{Stage<T>, ScopedStage<T>, StageHost, Rest}` are public so `ulo-grpc` runs
  `PreDispatch<Grpc>` entries without duplicating the stage (Q14); HTTP's own run goes through the
  same `Stage<Http>`; `PreDispatch<T>`'s `Default` is written by hand [S 11]. Read:
  `ulo-http/src/lib.rs:50-51`; `ulo-http/src/pre_dispatch.rs:75, 87, 314, 326, 364, 482`.
- `ulo-http-hyper` lost `listener.rs` to `ulo-hyper-serve`; `convert.rs` builds a request's
  `ConnInfo` from the accepted connection's [S 12].

### WebSocket [W]

- The read loop flushes right after reading a Ping or a Close, since tungstenite queues the Pong
  and the Close reply and writes them only on the next read or flush, and the loop stops reading
  under `max_inflight` and in the drain. After a Close in either direction the loop keeps reading,
  data frames ignored, bounded by `pong_timeout` [W 3]. tungstenite 0.28 returns protocol, UTF-8 and
  capacity errors from `read` without writing a Close, so the loop writes it: 1002, 1007, 1009,
  each `ProtocolError`; `ResetWithoutClosingHandshake` is `Lost`; the frame limit is
  `min(16 MiB, message_limit)`. The design's sentence about tungstenite answering 1009 itself does
  not match the crate [W 4]. Read: `ulo-ws/src/connection.rs:666-680, 832-842, 916-922`.
- A message with an `id` whose handler returns nothing is answered `{"id":..,"complete":true}`;
  a streamed answer to a fire-and-forget message writes its items and `complete` without an `id`;
  `cancel` fires `ClientCancelled` on every message in flight with the id, `cancel` without an id
  answers `bad_request`, an unknown id is ignored [W 7, W 8, W 9]. Read: `ulo-ws/src/envelope.rs:404-410`.
- Defaults: `max_inflight` 64, `max_outbound` 1024, `max_connections` unbounded; `message_limit`
  64 MiB and keep-alive 30 s as designed. A message counts against `max_inflight` until its answer
  is known; a streamed answer still writing holds no slot but the drain waits for it; stream items
  wait for room in the outbound queue, and the overflow policy governs what cannot be held back
  (one-frame replies, error envelopes, broadcasts, `Connection::send`) [W 10, W 11].
- Each codec's other frame kind closes with 1003, text on a MessagePack gateway included; a
  hand-written gateway receives both kinds [W 12]. `ConnectRefused::kind` maps `BadRequest`,
  `NotFound`, `Conflict` and `Unprocessable` to 1008, `Timeout`, `Unimplemented` and any later
  kind to 1011; a reason over 123 bytes is cut at a character boundary [W 13]. `refuse =
  handshake`'s 401 carries `WWW-Authenticate: Bearer`, the HTTP server's configured challenge
  being unreachable from the hand-off [W 14]. Read: `ulo-ws/src/connection.rs:438`.
- The `max_connections` slot is taken after the 101 under `refuse = close` and after admission
  under `refuse = handshake`, so a refusal counts against no limit; over it the connection closes
  1013 before any hook [W 28]. A streamed answer's `Completed` is reported once its `complete` is
  queued, not written [W 29]. Read: `ulo-ws/src/connection.rs:481`.
- A MessagePack message's `id` is kept as the JSON text of the scalar it decoded to and written
  back as that scalar, equal by value; JSON ids are echoed byte for byte through `raw_value`;
  `data` stays raw until a `Payload<T>` reads it; `"data": null` is present and an absent key is
  `Missing` [W 21]. The `Payload` probe is a no-op and `#[message]` emits no probe call, since the
  data stays undecoded until read [W 22]. `NodeId::current` is two `RandomState` hashes over the
  process id and the clock, no random-number crate being a dependency [W 20]. Read:
  `ulo-ws/Cargo.toml:33`; `ulo-ws/src/__private.rs:45`.
- Handshake refusals: a method other than GET answers 405 with `Allow: GET`; a missing
  `Connection: Upgrade`, `Upgrade: websocket` or `Sec-WebSocket-Key` 400; a version other than 13
  426 with `Sec-WebSocket-Version: 13`; a request during the drain 503; on the HTTP port a request
  whose backend handed no upgrade future 400 [W 26]. Read: `ulo-ws/src/connection.rs:427, 532, 545`.
- `master`'s `ulo::ws`, `BroadcastService` and `ulo-ws-redis`'s Redis-set membership are not
  carried over (§4.3, R10) [W, Retired].

### RPC core and the socket links [R]

- `ulo-rpc` depends on tokio with `rt`, `sync` and `macros` (per-call tasks in a `JoinSet`, the
  client's reply router) and `bytes` gains `serde`, so `Bytes` travels as a CBOR byte string [R 8].
  Read: `ulo-rpc/Cargo.toml:18, 26`.
- `serve` spawns each call into a `JoinSet` and routes `in`, `in_end` and `cancel` synchronously
  in arrival order; the inbound stream ending before the drain returns `Err`, which starts the
  shutdown; `close` aborts the calls, then calls `Link::close`; a handler value not built by the
  RPC attributes is refused in `prepare` [R 20]. Read: `ulo-rpc/src/server.rs:73`.
- A `cancel` for `ClientCancelled` or `Disconnected` writes nothing; `Drain` and `Explicit` write
  `err` `unavailable` [R 15]. The streamed request's items reach the handler over an unbounded
  channel, since a bound needs the reserved `credit` window and blocking the serve loop would stall
  every call on the link [R 16]. Over `max_inflight` the shed `err` `unavailable` carries a
  `RetryAfter` detail of one second, `Admission`'s default, and `Server` has no `shed_retry_after`
  [R 17]. `ErrorInfo`'s domain is `"ulo.rpc"` [R 18]. Read: `ulo-rpc/src/dispatch.rs:34, 153,
  418-422`.
- The frame is a map with `t` first; `h` is a map in order, a repeated name written once per
  value; an empty payload is written as no `d` and a missing or `null` `d` reads as empty;
  `e.details` is `Details`' stable JSON parsed back by tag, an unknown tag kept as `Detail::Json`;
  an unknown `kind` reads as `internal` [R 22]. `credit` is `{t:"credit", id, n}`, reserved: the
  server ignores it and no link sends it [R 21]. Read: `ulo-rpc/src/frame.rs:15, 46, 64`.
- On a JSON link a payload is serialized through a formatter that notes `write_byte_array`; one
  that wrote raw bytes is refused `BadRequest`/`binary_unsupported` before any I/O; `Codec::encode`
  itself still writes bytes as an array of numbers [R 23]. The Redis whole-frame carriage shares
  the public `Codec::{encode_frame, decode_frame}`; TCP adds only its 4-byte prefix [R 24]. Read:
  `ulo-rpc/src/codec.rs:130-135`; `ulo-rpc/src/client.rs:179`.
- Client: one reply-router task per connection, a reply for an id no call waits on dropped (the
  `FanOut` duplicate rule); a lost reply lane fails the waiting calls `Unavailable` and the next
  call connects again; a `goaway` keeps the calls in flight and sends the next over a new
  connection; `cancel` on drop is sent from a spawned task and not at all outside a tokio runtime;
  `.within(&exec)` stamps `deadline-ms` with the remaining time, refuses a passed deadline as
  `Timeout` before I/O, and bounds the wait by the shorter of the timeout and the remaining time;
  a stream call answered by a single `res` yields that value and ends; a failure in a streamed
  request's items fails the call and sends `cancel` [R 25]. `RpcClientModule`'s identity is a
  per-`for_root` instance labelled `RpcClientModule`, so two clients of one link type are two
  modules; the client is a singleton reading `Dep<dyn Timer>`, so an app without a `Timer` fails
  `wire()` [R 26]. Read: `ulo-rpc/src/client_module.rs:50-73`.

### The broker links and the RPC conformance suite [B]

- Where a miss signal arrives after `send` returned, the link writes the `err`: NATS's
  no-responders is a 503 status message on the reply subject and MQTT's PUBACK/PUBREC 0x10 arrives
  after the packet left, so both answer on `replies` with `err` `unavailable`,
  `ErrorInfo { reason: "no_destination", domain: "ulo.rpc" }`; Redis, RabbitMQ and Kafka fail
  `send` with `NoDestination`. An `evt` is published without `mandatory` on AMQP and its `PUBLISH`
  count is not read on Redis, so an event to nowhere succeeds [B 5]. Read:
  `ulo-rpc-redis/src/link.rs:385-390`.
- The drain: NATS drains each pattern subscription; AMQP cancels each consumer; MQTT unsubscribes
  each `$share` filter and resubscribes only the control topic after a reconnect; Kafka pauses its
  assignment and commits asynchronously; Redis unsubscribes its patterns. The control lane stays
  open and the inbound stream ends once draining holds no call, since a held call can still
  receive its items or a `cancel`; `close` closes the connections and commits synchronously on
  Kafka [B 9].
- MQTT: rumqttc's `publish` returns before a packet id exists, so the client serialises its
  publishes and waits for each one's `Outgoing::Publish(pkid)` event before the next, so a PUBACK
  names its call; sessions are clean; the URL is parsed by the link (`mqtt`, `tcp`, `mqtts`,
  `ssl`, `user:password@`, bracketed IPv6), since rumqttc's parser sits behind an optional
  dependency and requires a `client_id` query; each connection's client id is `ulo-` plus 16 hex
  characters; packets up to 268,435,455 bytes are accepted; rumqttc acknowledges each incoming
  QoS 1 or 2 publish itself, so the `Ack` is a no-op [B 11]. Read: `ulo-rpc-mqtt/src/link.rs:66`.
- Kafka: offsets are stored once the handler settles the `Ack` and committed by auto-commit
  (`enable.auto.offset.store = false`); the reply consumer's group is the reply topic's name, read
  from `earliest`; a request and its control frames are keyed by the client instance's id, a reply
  by its correlation id; topics are created at `bind`, an existing one keeping its shape [B 10].
  Read: `ulo-rpc-kafka/src/link.rs:170`.
- TLS: async-nats needs a crypto provider to compile at all, so the crate enables `ring` and
  `tls://` works in every build; RabbitMQ and MQTT use their libraries' default rustls features;
  Redis has a `tls` feature (`tokio-rustls-comp`, webpki roots), without which `prepare` refuses
  `rediss://`; Kafka has a `tls` feature (`rdkafka/ssl`, system OpenSSL), an `ssl://` entry setting
  `security.protocol=ssl`, refused without the feature, and mixing `ssl://` with plaintext refused
  [B 13]. Read: `ulo-rpc-nats/Cargo.toml:20-22`; `ulo-rpc-redis/Cargo.toml:13-15`;
  `ulo-rpc-kafka/Cargo.toml:13-15`; `ulo-rpc-kafka/src/link.rs:292-322`.
- A request-lane message of an unknown `ulo-t` kind is rejected without requeue on AMQP and
  Kafka and dropped with a `warn` on NATS, MQTT and Redis; AMQP headers are a field table, so a
  name repeated in `CallHeaders` keeps its last value and incoming tables, arrays and byte arrays
  are left out [B 14].
- The suite: `cases/app.rs` holds three controllers, `CoreController` always,
  `StreamController` where the link's `shapes` carry every streamed shape, `BinaryController`
  where it declares `binary`, so a link refusing a shape in `prepare` still runs every scenario it
  carries; a streamed scenario on a link without streamed shapes asserts the startup refusal, the
  binary scenario on a JSON link the client's `binary_unsupported` and the startup refusal of a
  binary handler. Misses assert `Unavailable` with `pattern_unhandled` or `no_destination` where
  `miss_signal`, the client's `Timeout` otherwise; the unhandled-event scenario asserts the emit
  fails `Unavailable` or not at all and that the server still answers; cancel and deadline assert
  `ClientCancelled` and `Deadline` through a value dropped by the handler, whether the server drops
  its future or lets it finish; oversized asserts `payload_too_large` one byte over a declared
  `max_frame`, and either a round trip or that error at 8 MiB without one; the drain asserts a
  held call and a stream ending on `draining()` open at close and a new call `Unavailable` or
  `Timeout`; two instances assert ten events delivered ten times under `Competing` and twenty under
  `FanOut`; `Broker::start` must answer a broker or namespace no other scenario shares [B 15].
  Not written, under rule 2: the per-crate `tests/conformance.rs` and the `integration` features
  [B, Replaced]. Read: `ulo-rpc-conformance/src/cases/app.rs:172-292`.

### gRPC and the build step [G]

- Health: the known services are the services the handlers' paths name; `bind` sets each
  SERVING, overwriting what an init hook set; `drain` sets the overall status `""` and each known
  service NOT_SERVING before the accept loop's drain, so a client polling health stops sending
  before the GOAWAY [G 14]. Without `GrpcHealth::module()` the server keeps a reporter of its own
  and health still answers [G 13]. With reflection on and no descriptor set, the two reflection
  services serve describing only themselves; a set that does not decode is a `Configure` failure
  [G 15]. Read: `ulo-grpc/src/server.rs:35-38, 62, 72`; `ulo-grpc/src/reflection.rs:11-20`;
  `ulo-grpc/src/health.rs:27-28`.
- `outgoing(exec, request)` sets tonic's `Request::set_timeout` from the deadline less
  `tokio::time::Instant::now()`, at least one nanosecond, since an `ExecutionRef` reaches no
  `Timer` and `ulo-tokio`'s clock is the same one [G 17].
- `grpc-status-details-bin`: `ErrorDetails` holds one of each message, so every `FieldViolations`
  detail's violations go into the one `BadRequest` and every `Help` detail's links into the one
  `Help`; the first `ErrorInfo` and the first `RetryAfter` are kept; `Detail::Json` values are
  appended to the `google.rpc.Status` tonic-types wrote; no details, no trailer [G 19].
- `ulo-build`: `compile` builds a `prost_build::Config` with the vendored `protoc` unless `PROTOC`
  is set, a `ServiceGenerator` wrapping tonic-prost-build's with `build_server(false)`, no server
  trait being implemented by anything; after each service it appends the marker module named by
  tonic-build's own snake-casing and, with clients on, the `GrpcClient` impl; a message path
  prost made relative gains `super::`; `finalize_package` appends `FILE_DESCRIPTOR_SET` as
  `include_bytes!` of an absolute path, since a relative one resolves against `OUT_DIR` [G 20].
  Read: `ulo-build/src/lib.rs:16, 41-83`; `ulo-build/src/markers.rs:3, 14-18, 30-37, 53, 62`.

### GraphQL [Q]

- async-graphql's context is inserted as `Dep<C>` plus the `ExecutionRef`, since the resolver
  hands back a shared value and `C` is not `Clone` [Q 5]; a response with `data` `null`, at least
  one error and no error carrying a `path` reads as `RequestError`, anything else `Executed`,
  which is how async-graphql answers parse, validation and operation-selection failures [Q 6];
  extensions are converted to the engine's value type, one that does not convert answering a
  request error, and juniper drops them [Q 10]. Read: `ulo-graphql-async-graphql/src/lib.rs:139,
  147-150`.
- juniper: one future owns the root node, the context and the request, calls
  `resolve_into_stream`, and forwards each response through a bounded channel; the returned stream
  is `select(receiver, driver)`, so polling it drives the future and nothing is spawned; each root
  field's stream yields `{field: value}` per event, several merged as they arrive;
  `NotSubscription` runs the operation through `execute`, so a query over graphql-transport-ws
  works [Q 7]. The root node is stored erased as `Arc<dyn Any + Send + Sync>` and downcast per
  call, since naming `RootNode<'static, Q, M, Sub>` in a field needs juniper's bounds on the struct
  [Q 8]; `sdl` needs the `schema-language` feature [Q 9]. Read:
  `ulo-graphql-juniper/src/lib.rs:42, 98-107, 128, 176`.
- A POST whose `Content-Type` is not `application/json` returns
  `ExtractError::UnsupportedMediaType`, rendering 415 with `Accept`; the body goes through
  `Bytes`' extractor, so over the route's limit is 413; a body that is not a GraphQL request and
  `variables` or `extensions` query parameters that are not JSON objects answer request errors
  [Q 15]. The playground is served on a GET with no `query` when `playground` holds and the client
  names `text/html` with a nonzero quality, a wildcard not counting since `curl` sends `*/*`; the
  paths handed to `Engine::playground_html` carry `HttpCx::mount_prefix` [Q 16]. Paths are
  normalized at registration to one leading `/` and no trailing one [Q 17]. The GET-mutation 405
  carries `Allow: GET, HEAD, POST, OPTIONS`, found by a top-level scan of the document that skips
  comments and strings; when the scan cannot tell, the request executes [Q 14]. Read:
  `ulo-graphql-http/src/controller.rs:75-98, 109, 252-255`.
- The graphql-transport-ws gateway: the protocol state lives in the session, `ConnectionInit`
  holding the payload in a `OnceLock` and the acknowledged flag and running operations behind an
  `Arc` [Q 21]; `on_connect` refuses 4406 when the handshake echoed no `graphql-transport-ws`; the
  init clock is a spawned sleep on the app's `Timer`, 4408 at expiry; a second `connection_init`
  closes 4429; `ping` answers `pong` echoing its payload; a binary frame, unparseable text, an
  unknown `type` or a malformed `subscribe` closes 4400; `subscribe` before the ack 4401, a running
  `id` 4409; an `Executed` response is `next`, a `RequestError` is `error` ending the operation
  without `complete`; the drain writes `complete` and ends the operation; the client's `complete`
  cancels `ClientCancelled` and a disconnect `Disconnected`; an `id` may be reused once its
  operation has ended [Q 22]. Read: `ulo-graphql-ws/src/gateway.rs:59-66, 188, 201-215, 232, 253`.
- Not carried from `master`: `GraphQLContextBuilder`, async-graphql's own WebSocket driver,
  juniper's Playground page, GraphQL batch requests [Q, Replaced].

### The development command [D]

- Which binary a build runs, a change during a build, and the environment are in U31. `main` is
  a plain `fn` dispatching `__exec` before any runtime exists and building a tokio runtime for
  every other command; the trampoline sets `LISTEN_PID` through `Command::env` and execs with
  `CommandExt::exec`, never calling the `unsafe` `set_var`; `#[command(name = "__exec", hide =
  true)]` keeps it out of `--help` [D 17]. `place_listen_fd` is the only hook the command installs;
  the grouped spawn adds `setpgid`, which lets the stop reach anything the application spawned
  [D 12]. A child that exits on its own is not reported; the next successful build starts it again
  [D 18]. `--listen 0.0.0.0:0` holds a port the OS chooses and an endpoint written `0.0.0.0:0`
  adopts it [D 5]. Read: `ulo-cli/src/main.rs:22, 26`; `ulo-cli/src/commands/exec.rs:1, 22, 42`.
- Owed after the race: `master`'s `socket_handoff` tests for `place_listen_fd` and the
  `cargo_invocation` tests against `build_args()`; tests for `dev_socket_name`, `resolve` under
  `ULO_DEV=1`, `listen_addr`'s refusals and `pick`; `commands/new.rs`, `commands/generate.rs` and
  `templates/` target the old API [D, Owed].

### The embedding adapters and the HTTP suite [E]

- salvo 0.92 has no `serve_with_graceful_shutdown`; `run` takes `server.handle()`, serves with
  `try_serve`, and calls `stop_graceful(drain)` once `stopping()` resolves, polling the serve
  future throughout; `salvo` gains the `server-handle` feature [E 1]. Every `run` closes the app
  when `Handle::host` refuses the host, since the app is bound by then [E 3]. poem's
  `take_upgrade` error crosses as its `Display` [E 13]. Read: `ulo-http-salvo/src/run.rs:12,
  28-34`; `ulo-http-salvo/Cargo.toml:22`; `ulo-http-poem/src/endpoint.rs:35`;
  `ulo-http-axum/src/run.rs:39`.
- rocket: `/<path..>` for all nine methods at rank 100, above rocket's default ranks; the cache
  holds `Option<Routing>`, nothing cached for a forwarded miss; the version is HTTP/1.1 and
  `ConnInfo::local` unset, since rocket's `Request` exposes neither; the upgrade hand-off carries
  the protocol the app's 101 names [E 7, E 8]. A rocket body of exact size is handed over sized so
  rocket writes `Content-Length`; salvo and poem wrap the body in a mutex-guarded `Sync` body
  keeping its size hint; salvo always gets a body, since it replaces an error status with no body
  by its catcher's page [E 9]. Read: `ulo-http-rocket/src/handler.rs:24, 46, 76`.
- The suite compares against a `HyperHost` started in the same mode: status, nine headers the app
  writes and the body bytes, framing and host headers left out [E 16]. The drain scenario: on
  HTTP/1.1 half a request head before `close` and the rest once `draining()` resolves, expecting
  503 with `Connection: close`; on HTTP/2 a held stream across the drain then a new stream that
  must not be served 200, GOAWAY itself unobserved [E 18]. The disconnect scenario: `/endless`
  writes one event, idles 600 ms, then writes every 100 ms; `AtClose` must observe the disconnect
  by 300 ms, `AtNextWrite` by 3 s [E 19]. The upgrade scenario echoes raw bytes after a 101 naming
  `websocket`; `tokio-tungstenite` was dropped from the suite [E 20]. Read:
  `ulo-http-conformance/src/reference.rs:21`.

## 4. Diagnostics and error text

- `#[message]` on an impl without `#[ulo_ws::gateway]` fails E0277 with the note "add
  `#[ulo_ws::gateway(path = "/..")]` to the `#[routes]` impl, or implement `ulo_ws::Gateway` by
  hand" [W 27]. `#[message]` with no argument: "#[message] takes the event, as in
  #[message(\"chat.send\")]". `session` beside `session_with`: "two spellings of one setting;
  write one" [W]. Read: `ulo-ws/src/gateway.rs:39-40`; `ulo-ws-macros/src/message.rs:29`;
  `ulo-ws-macros/src/gateway.rs:237`.
- WebSocket `prepare` refusals, each naming the gateway's controller through `Failure::naming`:
  the event field `id` or `data`; a gateway path not starting with `/` or holding a `{param}`; two
  handlers for one event; message handlers on a hand-written gateway ("it implements
  `ulo_ws::Gateway` by hand, which reads every message raw, so its `#[message(..)]` .."); a
  standalone server with no `port = own` gateway ("`ulo_ws::Server` serves the gateways declared
  `port = own`, and none is; .."); `.message_limit(0)` "would close every connection with 1009 at
  its first message; leave it unset for the 64 MiB .."; the five zero limits through
  `zero_count`/`zero_bound` with their effects ("close every connection with 1013 as it opens",
  "stop reading every connection before its first message", "overflow every connection at its
  first outbound message", "ping every connection without pause", "end every connection at its
  first Ping") [W 25]. Read: `ulo-ws/src/gateway.rs:710, 721, 781-793`; `ulo-ws/src/server.rs:251`.
- RPC: a handler value not built by the attributes: "<handler> was not mounted by
  `#[ulo_rpc::message]` or `#[ulo_rpc::event]`"; a non-unary event: "<handler> (pattern `p`) is an
  `#[event]` handler with a <shape> shape; an event is one payload answered by nothing"; a stream
  return on `#[event]`: "an `#[event]` handler answers nothing; a stream return makes it a
  streamed reply, which `#[message]` declares"; `.max_frame(0)` "would refuse every frame";
  `RpcClientModule::timeout(Bound::After(Duration::ZERO))` on the <link> link "would time out every
  call; write `Bound::Unbounded` to turn the timeout off"; a stream on a link without the shape:
  "the udp link carries no streamed reply" at runtime [R 12, R 20, R 26, R 27]. Read:
  `ulo-rpc/src/server.rs:73, 82`; `ulo-rpc-macros/src/message.rs:69`;
  `ulo-rpc-tcp/src/link.rs:158`; `ulo-rpc/src/client_module.rs:59-63`.
- Brokers: a `.group(..)` the broker would refuse fails `prepare` naming the setting; MQTT's
  CONNACK without shared subscriptions: "the MQTT broker's CONNACK announces no shared-subscription
  support, so the link's .." naming `Mqtt::group`; a SUBACK refusing a filter names the filter;
  the event loop ending first: "the MQTT link's event loop ended before the broker answered";
  Kafka: "the Kafka link's brokers mix `ssl://` and plaintext entries", "the Kafka link's brokers
  ask for `ssl://`, which needs the crate's `tls` feature"; a miss: "nothing listens on pattern
  `p`" [B 5, B 8, B 11, B 13]. Read: `ulo-rpc-mqtt/src/link.rs:168, 431`;
  `ulo-rpc-kafka/src/link.rs:300, 313`.
- gRPC: a parameter or reply whose message type or shape differs from the marker's is E0599 at
  `ParamCheck::matches` or `Checked::checked`, the type naming both sides
  (`Checked<(Shaped<true>, UserEvent), (Shaped<false>, User)>`); `max_inflight`,
  `max_per_connection` and `max_concurrent_streams` at `Count::Max(0)` and `handshake_timeout` at
  zero are refused in `prepare` with their effects ("shed every call", "shed every call on every
  connection", "let no HTTP/2 stream open", "drop every TLS connection before its handshake");
  "reflection: a file descriptor set does not decode: .."; a `grpc-timeout` off the grammar is
  logged at `debug` [G 3, G 11, G 15]. Read: `ulo-grpc/src/__private.rs:13-16`;
  `ulo-grpc/src/server.rs:149-152`; `ulo-grpc/src/reflection.rs:19-20`;
  `ulo-grpc/src/dispatch.rs:366`.
- GraphQL: `connection_init_timeout` at zero "would close every connection before its
  `connection_init` could arrive"; "the request's context could not be built" (the lookup error at
  `error`); "a mutation is not executed over GET; send it as a POST"; the close reasons "Subprotocol
  not acceptable", "Connection initialisation timeout", "Too many initialisation requests",
  "Invalid message received", "Subscriber for {id} already exists" shortened past 123 bytes
  [Q 4, Q 14, Q 19, Q 22]. Read: `ulo-graphql-http/src/module.rs:66-70`;
  `ulo-graphql-http/src/controller.rs:255`; `ulo-graphql-ws/src/gateway.rs:203, 215, 232, 253`.
- Embedding: a request outside the prefix logs once at `warn`, "a request outside the embedding's
  `.nested_at` prefix is answered as the app's 404", with `host`, `path` and `prefix` fields; a host
  refused by `Handle::host` closes the app with a `Signal` naming it ("the <host> host server was
  refused") [50b8439c, E 3]. Read: `ulo-http/src/embed.rs:861-867`; `ulo-http-axum/src/run.rs:39`.
- `ulo dev`: `--listen` refusals name the bare port, `udp://` and the non-address each with their
  own text; several binaries: "`cargo build` produced N binaries (..); choose one with `--bin`, or
  name it in the package's `default-run`"; `__exec` without `--`: "`ulo __exec` needs the command
  to run after `--`"; off Unix: "`ulo __exec` hands inherited sockets to its child, which needs
  Unix"; under `ULO_DEV=1` with no adoptable socket a `debug` or `warn` line says why the address
  binds as written [D 3, D 8, D 15, D 17]. Read: `ulo-cli/src/commands/dev.rs:261-267, 475`;
  `ulo-cli/src/commands/exec.rs:22, 42`; `ulo-net/src/endpoint.rs:136-145`.
- `ulo-handler-codegen`: a `#[meta]` value naming another transport fails E0308 at the value,
  `expected (), found MetaMismatch<Http, Rpc>` [S 5]. Read: `ulo/src/__private.rs:127`.

## 5. Internal only

- `MetaSame`, `MetaOther<T, U>` and `MetaAny` are imported anonymously as the reply probe's arms
  are; the probe's value is bound to `__ulo_value` (mixed-site) [S 5]. `emit::metadata(meta,
  paths)` takes the transport's `Paths` for the marker [S 6]. Read:
  `ulo-handler-codegen/src/emit.rs:270-289`.
- `Codec::of(&Capabilities)` is crate-private; `DOMAIN` is `"ulo.rpc"`; each broker server keeps a
  correlation-to-local-id table released on the terminal reply or `cancel`; the Redis link's
  `base` is `Uuid::new_v4().as_u64_pair().0` [R 6, R 18, B 3, B 4]. Read: `ulo-rpc/src/codec.rs:47`;
  `ulo-rpc/src/dispatch.rs:34`; `ulo-rpc-redis/src/link.rs:101-103, 350, 377`.
- `GrpcHandler { path, call }` with `new::<M, F>(call)` unchanged; `MESSAGE_LIMIT` is a
  `pub(crate) const`; `render` lives in `status.rs` [G 12, G 18, G 8]. Read:
  `ulo-grpc/src/__private.rs:41-47`; `ulo-grpc/src/extract.rs:18`; `ulo-grpc/src/status.rs:70`.
- `ConnectionInit`'s two fields are `pub(crate)`; `EndpointSettings<Q>` and `InitTimeout` are
  module-private bindings; `GraphqlModule<Q>` and `GraphqlConfig<Q>` implement `Clone`,
  `PartialEq`, `Eq` and `Hash` by hand so `Q` carries no bound, the identity being
  `ModuleIdentity::of_value(self)` [Q 20, Q 21]. Read: `ulo-graphql-http/src/module.rs:57-62,
  77-96`; `ulo-graphql-ws/src/gateway.rs:59-60`.
- `Shared.prefix: OnceLock<String>` is set at `prepare` and read only under
  `STRIPS_PREFIX == false`; `outside_warned: AtomicBool`; `OutsidePrefix` is `pub(crate)` and
  `Copy` [50b8439c]. Read: `ulo-http/src/embed.rs:661-669`.
- `HubMeta` is `pub(crate)` module metadata; `SharedAdapter` wraps the configured adapter for the
  `also_as` binding; `join_path` and `check_defaults` are `pub(crate)` in `gateway.rs`; the read
  loop's close codes map in `connection.rs` [W 24, W 6, W 5, W 4]. Read: `ulo-ws/src/module.rs:118-135`;
  `ulo-ws/src/gateway.rs:761, 808`; `ulo-ws/src/connection.rs:916-922`.
- Workspace: `actix-http`, `actix-service`, `prost-build`, `watchexec-events`, `-signals`,
  `-filterer-ignore` and `ignore-files` added beyond the plan's list; `default-features = false`
  on `async-nats`, `salvo`, `poem`, `actix-web`, `actix-http`, `rocket`, `async-graphql`, `tonic`
  and `tokio-tungstenite`, each member enabling what it uses; `redis` gains `aio` and `tokio-comp`
  [S 14]. The rewritten crates lost their `tests/`, `examples/` and `README.md` (`ulo-cli` keeps
  its README); `commands/mod.rs` gains `pub mod exec` under the `dev` feature [S 15, S 44].
  `ulo-ws` enables `serde_json/raw_value`, `futures-util/sink` and adds `http-body-util`;
  `ulo-ws-redis` enables `redis/tokio-rustls-comp` and `tokio/time`; `ulo-rpc` enables
  `tokio/{rt,sync,macros}` and `bytes/serde`; `ulo-grpc` enables tonic's `tls-ring` and
  `tls-native-roots`; `ulo-graphql-juniper` enables `schema-language`, `ulo-graphql-async-graphql`
  `graphiql`; `ulo-http-salvo` enables `server-handle` [W, R 8, G 16, Q 9, E 1]. Read:
  `Cargo.toml:67-136`; `ulo-cli/src/commands/mod.rs:1-6`; `ulo-ws/Cargo.toml:23-33`;
  `ulo-ws-redis/Cargo.toml:20-23`; `ulo-rpc/Cargo.toml:18, 26`; `ulo-grpc/Cargo.toml:33`;
  `ulo-graphql-juniper/Cargo.toml:17`; `ulo-graphql-async-graphql/Cargo.toml:16`;
  `ulo-http-salvo/Cargo.toml:22`.

## 6. Applied by the coordinator after the race, and the requests still open

Two commits followed the race:

- **`50b8439c`** (`crates/ulo-http/src/embed.rs`, `src/service.rs`): E's three requests, as U27
  describes them. `Embed::STRIPS_PREFIX` with its doc; `Embed::builtin_forwards` with its doc,
  `Embedded::new` returning `A::builtin_forwards(Embedded { .. })`; `Shared.prefix` set at
  `prepare` from the normalized mount; `Shared.outside_warned`; the `pub(crate) struct
  OutsidePrefix`; `Handle::body_limit()`; `Service::respond` calling `strip_prefix` when
  `!A::STRIPS_PREFIX`; `strip_prefix` rewriting `path_and_query` and inserting `OutsidePrefix` with
  a once-`warn` for a path outside; `ServiceInner` answering `Miss::NotFound` when the extension is
  present, before the router runs. Read: `ulo-http/src/embed.rs:85-96, 273-274, 276, 569,
  661-669, 753, 845-880`; `ulo-http/src/service.rs:207-209`.
- **`5d2ad695`** (`Cargo.lock` and four sources): the async-graphql subscription stream now goes
  through `execute_stream_with_session_data(request, Default::default())`, since
  `execute_stream` borrows the schema for its stream and the session-data form owns a clone, which
  a stream outliving the closure needs; four unused imports removed (`Future` in
  `ulo-grpc/src/dispatch.rs`, `Body as _` in `ulo-http-poem/src/endpoint.rs`,
  `ulo-http-rocket/src/handler.rs` and `ulo-http-salvo/src/handler.rs`); the lockfile rewritten by
  the compile, fetching `ciborium`, `rmp-serde`, `graphql-parser` and `void`, which the offline
  registry lacked [S 14, Q 9]. Read: `ulo-graphql-async-graphql/src/lib.rs:67-72`;
  `Cargo.lock:861, 2017, 4158, 6220`.

Cross-area requests the areas closed themselves: R's unique-ids request to B is B 3; D's two
requests to R are R 29; Q's prefix-join request to W is W 6; the spine's open points for W, R and
G are W 1, R 1 and G 10 (U1); S's `grpc_code` routing is G 1 (U22); B's prefetch request to the
design is U5. Still open, in U33: Q's `MethodNotAllowed::new`, D's `Activation::get` doc, R's
`Paths::transport` doc, W's retirement of `ulo-ws-tungstenite`, the unused `watchexec-events`
entry, and E's optional `HostPrefix` (U27).

## 7. The fold's eight choices, checked against the code

The fold of `REVIEW_2B.md` (commit `3606b431`) wrote eight things the twentieth response's text
did not say. Each, as built:

1. **X24's `MetaMismatch`/E0308 mechanism and its number.** §2.5 and §11 number it X24 and name
   `fw::__private::{MetaProbe, MetaMismatch}` with the E0308 at the value. Built as written, with
   the arms' names, the `Send + Sync + 'static` bound and the local binding of U6; the number holds
   through `BUILD_PLAN_2B.md` and the spine. The mismatch arm compiled nowhere. Read:
   `ulo/src/__private.rs:102-157`; `transports/DESIGN.md:288, 1152, 1177`.
2. **The sealed `HttpCarried` bound on `PreDispatch<T>`.** §3.4, §11 and §12 say "sealed". Built
   as `HttpCarried: Transport + __private::Carried`, `Carried` public and doc-hidden: a seal by
   convention (U6). Read: `ulo-http/src/transport.rs:38-42`; `ulo-http/src/__private.rs:19-21`;
   `transports/DESIGN.md:479, 1151, 1178`.
3. **Redis carrying the whole frame.** §5.2, §5.3's table and §13 item 12. Built: the link
   publishes `Codec::encode_frame`'s bytes and reads them back with `decode_frame`; added beyond
   the fold, the `ulo-reply` header, the per-caller id offset and the `ulo:rpc:control` channel
   (U17). Read: `ulo-rpc-redis/src/link.rs:233, 295, 366, 388, 414`; `transports/DESIGN.md:850,
   915`.
4. **MQTT failing `bind` without shared subscriptions.** §5.2 and §12's row. Built: `listen` waits
   for the first CONNACK and every SUBACK, and a CONNACK whose Shared Subscription Available is 0
   fails `bind` naming `Mqtt::group`; after a reconnect the same CONNACK logs at `error` and the
   link keeps the control topic, which the fold does not state. Read:
   `ulo-rpc-mqtt/src/link.rs:160-175, 425-431`; `transports/DESIGN.md:854, 1198`.
5. **`WsModule` carrying the same-port gateway defaults.** §4.1. Built: `Defaults` on the module
   with one setter each, handed to `Handoff::new`, read by the hand-off's `prepare` through
   `build_table` and checked by `check_defaults` (U12). Read: `ulo-ws/src/module.rs:36, 59-91,
   107`; `ulo-ws/src/handoff.rs:99`; `ulo-ws/src/gateway.rs:761`; `transports/DESIGN.md:726`.
6. **`GraphqlConfig::engine::<Q>()`.** §7's two-engines paragraph. Built as
   `pub fn engine<E: 'static>(self) -> GraphqlConfig<E>`, the parameter carried by
   `GraphqlModule<Q>`, the private endpoint controller and `GraphqlWs<Q>`, read as
   `Dep<dyn Engine, Q>` [Q 20]. Read: `ulo-graphql-http/src/module.rs:36, 98, 144`;
   `ulo-graphql-ws/src/gateway.rs:47, 170`; `transports/DESIGN.md:1081`.
7. **The `Serve` API sketch.** §3.7 and the plan's row. Built with three signature differences,
   `new -> io::Result`, `run(&self)` and `Accepted.draining` (U7). Read:
   `ulo-hyper-serve/src/serve.rs:30-37, 77, 98`; `transports/DESIGN.md:540`.
8. **The name `SalvoRequest`.** §3.8's table and its note. Built as `pub struct SalvoRequest<'r>`
   holding the request and the `Depot`, `Salvo::HostRequest<'r> = SalvoRequest<'r>`, the
   `OriginalPath` forward reading `req.request.uri()`. Read: `ulo-http-salvo/src/lib.rs:41, 48,
   55`; `ulo-http-salvo/src/handler.rs:66`; `transports/DESIGN.md:646, 651`.

Five of the eight are built as folded (1, 3, 5, 6, 8); 2 and 7 diverge and are U6 and U7; 4 is as
folded with one unstated case.

## 8. What the compile showed

`cargo check --workspace --all-targets` passes on stable. On Rust 1.88 it passes for every crate
but `ulo-http-salvo` (`rust-version = "1.92"`, salvo's floor) and `ulo-graphql-async-graphql`
(`rust-version = "1.89"`); the workspace's `rust-version` stays `1.88`, so those two crates are
the MSRV exceptions and every other 2b crate inherits the workspace's. Read: `Cargo.toml:61`;
`ulo-http-salvo/Cargo.toml:11`; `ulo-graphql-async-graphql/Cargo.toml:5`.

A compile reaches a macro's generated code only where something expands it:

- **Expanded.** The HTTP verbs, `#[meta(BodyLimit(16))]`, `Valid<Json<Item>>` and an `Sse` reply
  by `ulo-http-conformance`'s app (`#[ulo_http::get]`/`post` on a `#[routes]` impl). RPC's
  `#[ulo_rpc::message]` and `#[ulo_rpc::event]` by `ulo-rpc-conformance`'s app, across the unary,
  server-streaming, client-streaming (`Inbound<i64>`), bidi and `Payload<Bytes>` shapes, so R's
  reply probe, its `TypeId` arm for `()`, the `InboundProbe` and the return-type read are
  type-checked. The hand-written `Gateway`/`GatewayConfig` path by `ulo-graphql-ws`'s
  `GraphqlWs`. X24's matching arm by the HTTP app. Read: `ulo-http-conformance/src/app.rs:88-143`;
  `ulo-rpc-conformance/src/cases/app.rs:172-292`; `ulo-graphql-ws/src/gateway.rs:188, 201`.
- **Not expanded.** `#[ulo_ws::gateway]` and `#[ulo_ws::message]` appear in 17 lines, all doc
  comments inside ```` ```ignore ```` fences; `#[ulo_grpc::method]` in 5, likewise; no workspace
  member has a `build.rs`, so `ulo_build::configure().compile(..)`, the generated markers, the
  `GrpcClient` impl and `FILE_DESCRIPTOR_SET` ran nowhere. Their generated code is unverified
  until a test crate uses them: W's `Answer<M>` inference at a `#[message]` call (W, Not
  verified), G's six-arm probe with a const-generic turbofish and the anonymous const inside a
  generic impl (G, Not verified), the `session_with` wrapping, and X24's E0308 arm. Read: grep over
  `crates/` for the three attributes outside `//`; `ulo-ws/src/lib.rs:4`;
  `ulo-grpc-macros/src/lib.rs:4`; `ulo-build/src/lib.rs:8`.

What the compile settled from the logs' "Not verified" lists: every `Send` bound the logs worried
about (W's read loop under `tokio::spawn`, R's and B's spawned futures, Q's operation tasks, E's
host futures); the juniper bounds and the `RootNode` covariance; `tonic::Streaming<T>: Unpin`;
the `select!` borrows in W's read loop; redis 1.3's, lapin 4.10's, rumqttc 0.25's and rdkafka
0.39's APIs as called; the coherence of G's `ReplyValue`/`ReplyStream` impls beside `Response<T>`;
`tokio::sync::watch::Sender::new`; D's `watchexec` closures and the `break 'session` inside
`select!`. What it cannot settle: everything a socket or broker shows, listed per area in the
logs (B's lapin confirm-plus-direct-reply-to, rumqttc's `Outgoing::Publish` ordering, librdkafka's
`assign` from `Offset::End` and the auto-create-off miss; E's disconnect and drain timings; D's
end-to-end handoff; G's GOAWAY, `REFUSED_STREAM` and hyper dropping a reset call's future), plus
serde's reading of a CBOR `null` `d` and of a JSON `null` into `Option<Box<RawValue>>`.

## 9. Before tests, and what can follow

**Decide before tests** (each changes code a test would pin):

- U2: the deadline on RPC and gRPC runs the error handlers under a `timeout_grace`, or does not.
- U5: the AMQP prefetch follows `max_inflight` through a defaulted `Link` method, and
  `RabbitMq::prefetch` goes.
- U26: `Outcome::Failed` for a context that cannot be built.
- U33: `MethodNotAllowed::new` public, so Q's 405 reaches the error handlers; delete
  `crates/ulo-ws-tungstenite` and its `exclude` entry; drop `watchexec-events`.
- U1: a yes on the three mechanisms, since every W, R and G test is written against them.
- U4: a yes on `wire()` for the three module-level refusals, since the RPC conformance suite's
  startup-refusal assertions read the error's variant.

**Can follow** (accept now, build later): the `Paths::transport` and `Activation::get` docs
(U33); `timeout_grace`'s zero refusal text once U2 is built; `message_limit` on
`ulo_grpc::Server` (U23); a `--stop-grace` flag (U31); a shape override on `#[message]` (U14); a
`Host` method for a host started before `listen()` (U30); E's `HostPrefix` (U27); client TLS on
TCP and UDP retries (U16); the ordering key on `RpcClient` (U20); per-operation enhancers on
graphql-transport-ws (U26); a report for a `port = own` gateway no server serves (U9); the
compile-fail test for X24's mismatch (U6); and the design amendments every "accept" above names
(§2.5, §2.6, §3.4, §3.7, §3.8, §4.1, §4.3, §5.1, §5.2, §5.3, §5.4, §6.1, §6.2, §7, §12, §13).

**Not verified here:** the runtime claims of §8's last paragraph; B's statement that no broker
link loses one caller apart from the others (U13), read from the logs and not from a broker; the
suite scenarios' assertions (B 15, E 16-20), read as written and not run; the twentieth
response's exact wording cited in U5 by range rather than line.
