# Build plan: race 2b

Race 2b builds the rest of the transport layer on the race 2a tree: WebSocket, RPC with its seven
links and their conformance suite, gRPC with its build step, GraphQL over both bindings, the five
embedding adapters with the HTTP conformance suite, and the development command. The core
extensions it needs are X19–X24 (`transports/DESIGN.md` §11).

The spine comes first and alone, as in 2a. It writes every new crate's module tree and every public
signature with `todo!()` bodies, so the contracts one area calls in another, `Gateway` for GraphQL,
`Link` for the brokers, `Method` for the build step, the embedding adapters' `Embed` impls, the
shared accept loop and the core extensions, are frozen before any area starts. That is what lets
the broker links begin beside the RPC core and the GraphQL gateway beside WebSocket. Seven areas
then fill it in parallel; each owns its files and builds against the signatures the others carry.

`transports/DESIGN.md` and `DESIGN.md` are the spec; `transports/RESPONSE.md` breaks a tie between
them, the latest response winning. `divergences/race2b-spine.md` lists every place the spine departs
from the designs or fills what they leave open; those entries await the user's sign-off like any
area's.

## Names

The designs write `fw` for the crate prefix; the workspace writes `ulo`. Where a design name and a
workspace crate differ by more than the prefix:

| Design | Workspace |
|---|---|
| `fw-hyper-serve` | `ulo-hyper-serve` (new) |
| `fw-http-conformance` | `ulo-http-conformance` (new) |
| `fw-ws`, `fw-ws-macros`, `fw-ws-redis` | `ulo-ws` (new), `ulo-ws-macros` (new), `ulo-ws-redis` |
| `fw-rpc`, `fw-rpc-macros` | `ulo-rpc` (new), `ulo-rpc-macros` (new) |
| `fw-rpc-amqp` | `ulo-rpc-rabbitmq` |
| `fw-grpc-build` | `ulo-build` |
| `fw-grpc-macros` | `ulo-grpc-macros` (new) |
| `fw-graphql`, `fw-graphql-http`, `fw-graphql-ws` | `ulo-graphql`, `ulo-graphql-http`, `ulo-graphql-ws` (all new) |
| `fw-graphql-async` | `ulo-graphql-async-graphql` |
| `fw-cli`, `fw dev`, `fw __exec`, `FW_DEV` | `ulo-cli`, `ulo dev`, `ulo __exec`, `ULO_DEV` |

Every other crate is the design name with the prefix swapped.

## Rules

The rules of `BUILD_PLAN_2A.md` apply unchanged, restated here with this race's names.

1. **No `cargo` in any form during the race**, and no git command. The user compiles once the
   stage is done, so each area reads its own files for type and borrow errors before handing them
   back. No area runs cargo mid-race to check its own work.
2. **No tests and no examples.** The two conformance suites are library crates and are written in
   the race; the per-crate `tests/conformance.rs` files that invoke them are written by the
   coordinator after the first compile.
3. **Own files only.** An area edits the files assigned to it below and no others. A change needed
   in another area's file, a new `pub(crate)` item or a changed signature included, is written as
   a request in `divergences/race2b-<area>.md` naming the file, the item and the reason; the
   coordinator routes it to the owner. The root `Cargo.toml` belongs to the coordinator: a new
   workspace dependency is a request too. An area owns its crates' `Cargo.toml` files.
4. **The contracts below are frozen.** An item listed under "Cross-area contracts" keeps its name,
   fields and signature. Its owner may add private helpers and fields no other area reads.
5. **Public signatures are frozen.** A public item keeps the signature the spine gives it.
   Adding, renaming or reshaping one is a divergence.
6. **Every divergence is logged** in `divergences/race2b-<area>.md`, one entry each: what the
   design says (or that it is silent), what was written, and why. A behaviour the design states
   and the code does differently is a divergence even when no signature changes. The log also
   records what the area replaced on `master` and what it could not carry over.
7. **Code style.** Rust 2024, stable, MSRV 1.88. The core depends on no async runtime. Comments
   say what the code cannot: why, ordering constraints, invariants; public docs say when to use an
   item and what the design states about it. No labels, no step narration, no section dividers.
8. **`todo!()` is the only panic the spine leaves**, and every one is replaced. The core and the
   macros never panic on a path the design gives a typed error.

## S. The 2b spine (first, alone)

The spine owns the core extensions, the two `ulo-http` and `ulo-transport` changes, the shared
accept loop, the `#[meta]` transport probe, the workspace manifest, and every new crate's skeleton.

| Files | Holds |
|---|---|
| `crates/ulo/src/transport/inputs.rs`, `crates/ulo/src/graph/wire.rs`, `crates/ulo/src/graph/mod.rs` | X19: `Inputs::also_seeded_by::<U>()` applying to the last `input` written; `InputDecl.seeders` as a set; the merge of two declarations of one key with equal seeder sets in `declare_transport_inputs`, `InputConflict` for any other pair |
| `crates/ulo/src/transport/server.rs`, `crates/ulo/src/app/handle.rs`, `crates/ulo/src/lib.rs` | X20: `Mounted::handlers_of::<U: Transport>()`, `AppHandle::mounted::<T: Transport>() -> Result<Vec<MountedHandler<T>>, TimerMissing>` (`mounted_parts` made reachable) |
| `crates/ulo/src/transport/metadata.rs`, `crates/ulo/src/__private.rs`, `crates/ulo/src/lib.rs` | X24: `TransportMetadata { type Transport: Transport }`; `__private::{MetaProbe<T, V>, MetaMismatch<T, U>}` with the three autoref arms |
| `crates/ulo-handler-codegen/src/emit.rs` | X24: `emit::metadata` emits `let (): () = (&&&MetaProbe::<T, _>::new(&value)).check();` per `#[meta]` value, spanned at the value, under the value's gates |
| `crates/ulo-http/src/limits.rs` | `impl TransportMetadata for Timeout` and `BodyLimit` naming `Http` |
| `crates/ulo-transport/src/prepare.rs`, `crates/ulo-transport/src/lib.rs` | X22: `Failures`, `Failure`, `Names`, `zero_bound(setting, bound, effect)`, `zero_count(setting, count, effect)`, public, moved from `ulo-http` |
| `crates/ulo-http/src/server.rs` | switched to X22's helpers; `Server<B>::drain` and `close` call every registered `UpgradeHandler`'s beside the backend's, concurrently; `prepare` calls each handler's `prepare` and `bound` |
| `crates/ulo-http/src/upgrade.rs` | X21: `UpgradeHandler::{prepare, bound, drain, close}`, defaulted |
| `crates/ulo-http/src/embed.rs` | X21: `Embedded<A>::drain` and `close` call the registered upgrade handlers' as `Server<B>` does |
| `crates/ulo-http/src/pre_dispatch.rs`, `crates/ulo-http/src/transport.rs`, `crates/ulo-http/src/lib.rs` | X23: `PreDispatch<T: HttpCarried = Http>`; the sealed `HttpCarried` marker, implemented for `Http` here and for `Grpc` in `ulo-grpc` through a `pub(crate)`-free sealing pattern the spine chooses (a `__private` supertrait) |
| `crates/ulo-hyper-serve/Cargo.toml`, `src/lib.rs`, `src/serve.rs`, `src/listener.rs`, `src/handshake.rs` | the shared accept loop: `Serve::new(listeners: Vec<BoundListener>, tls: Option<TlsAcceptor>, config: &ServeConfig)`, `Serve::run(self, connection: impl Fn(Accepted) -> Fut)` with `Accepted { io: Io, conn: ConnInfo, draining: Draining }`, `Serve::drain()`, `Serve::close()`, `ServeConfig { handshake_timeout: Option<Duration> }`, the accept back-off, the TLS handshake under its timeout, the per-connection `JoinSet`; moved out of `crates/ulo-http-hyper/src/{backend,listener}.rs` |
| `crates/ulo-http-hyper/src/backend.rs`, `src/listener.rs`, `Cargo.toml` | the hyper backend over `ulo-hyper-serve`: `bind` builds the `Serve`, `serve` runs it with a closure driving hyper's auto or `http1` builder, `drain` and `close` forward |
| `Cargo.toml` | `members` gains every crate below; `exclude` loses the ones brought back; the new workspace dependencies (tokio-tungstenite, async-nats, redis's `aio` features, lapin, rumqttc, rdkafka, tonic, tonic-prost, prost, prost-types, tonic-health, tonic-reflection, tonic-types, tonic-prost-build, protoc-bin-vendored, ciborium, rmp-serde, async-graphql, juniper, salvo, poem, actix-web, rocket, watchexec and its filterers) |
| each new crate's `Cargo.toml` and `src/**` | the module tree and every public signature with `todo!()` bodies for `ulo-ws`, `ulo-ws-macros`, `ulo-ws-redis`, `ulo-rpc`, `ulo-rpc-macros`, the seven link crates, `ulo-rpc-conformance`, `ulo-grpc`, `ulo-grpc-macros`, `ulo-build`, `ulo-graphql`, `ulo-graphql-http`, `ulo-graphql-ws`, `ulo-graphql-async-graphql`, `ulo-graphql-juniper`, `ulo-http-axum`, `-salvo`, `-poem`, `-actix`, `-rocket`, `ulo-http-conformance`, and `ulo-cli`'s `commands/dev.rs` and `commands/exec.rs` |

A crate on `master` that an area rewrites in place is emptied to its skeleton by the spine: the old
`src/**` is removed and the new module tree written, so an area never reads old code as if it were
the spec. The spine keeps nothing of `master`'s implementation but what the design names as
carried over (§9's `place_listen_fd`).

Design sections: transports DESIGN §1, §2.5 (X24), §2.10 (X19), §3.4 (X23), §3.5 (X21), §3.7
(`fw-hyper-serve`), §4.1 (X20), §11, §12 (the compile rows X24 adds); DESIGN §6.4, §10.1 step 2,
§13.

## Areas and files

Every source file of the race's crates belongs to exactly one area. Each area lists the crates it
rewrites in place from `master`, the excluded workspace members it brings back, and the crates it
creates.

### W. WebSocket

| Files | Holds |
|---|---|
| `crates/ulo-ws/Cargo.toml`, `src/lib.rs`, `src/transport.rs` | `Ws`, `WsConnect`, `WsCx`, `ConnectCx`, `Reply`, `NoHandler`, the inputs `ConnectionInfo`, `UpgradeHead`, `SessionHandle` declared by both markers (X19) |
| `crates/ulo-ws/src/gateway.rs` | `Gateway` (the trait a hand-written gateway implements), `GatewayConfig`, `GatewayRef`, `OnConnect`, `OnDisconnect`, `AfterInit`, `DisconnectReason`, `ConnectRefused` with `code(..) -> Result<Self, CloseCodeError>`, the `subprotocols` setting |
| `crates/ulo-ws/src/session.rs` | `Session<T>`, the session factory |
| `crates/ulo-ws/src/envelope.rs`, `src/codec.rs` | the JSON envelope, `Payload<T>`, `Frame`, the reserved `cancel`, the MessagePack codec over `rmp-serde` |
| `crates/ulo-ws/src/connection.rs` | the read loop, the outbound queue, `message_limit`, `max_inflight`, `max_outbound`, close codes, `ping_interval` and `pong_timeout` on the app's `Timer`, the per-message execution, `open_terminal` on disconnect, the `DisconnectReason` mapping |
| `crates/ulo-ws/src/handoff.rs` | the `ulo_http::UpgradeHandler` impl with X21's four methods, reading gateways through `AppHandle::mounted::<Ws>()` and `::<WsConnect>()` (X20) |
| `crates/ulo-ws/src/server.rs` | the standalone `Server` over `ulo-hyper-serve` and hyper `http1` with upgrades; `prepare` pairing `handlers()` with `handlers_of::<WsConnect>()`; the zero refusals through X22 |
| `crates/ulo-ws/src/rooms.rs`, `src/broadcast.rs` | `Rooms`, `RoomsIn`, `BroadcastAdapter`, the in-memory adapter |
| `crates/ulo-ws/src/module.rs` | `WsModule::for_root()`, `.broadcast(..)`, the gateway defaults; the `Upgrades` registration, the `Rooms` and adapter bindings |
| `crates/ulo-ws/src/__private.rs` | `WsHandler`, the `Payload` probe |
| `crates/ulo-ws-macros/Cargo.toml`, `src/lib.rs`, `src/gateway.rs`, `src/message.rs` | `#[gateway]`, `#[message]` over `ulo-handler-codegen`, the `cancel` refusal |
| `crates/ulo-ws-redis/Cargo.toml`, `src/lib.rs` | the Redis `BroadcastAdapter` value, `Redis::url(..)` |

Replaces on `master`: the WebSocket half of the old `ulo` core (`ulo::ws`, gateways, `BroadcastService`)
and `ulo-ws-tungstenite`, whose role is `ulo-ws`'s standalone server; the `ulo-ws-tungstenite`
directory stays excluded until the coordinator removes it, and W's log records the retirement.
Rewrites in place: `ulo-ws-redis`. Brings back: `ulo-ws-redis`. Creates: `ulo-ws`, `ulo-ws-macros`.

Design sections: §2.9 (the `ws.event` span), §2.10, §3.5, §4, §10 (WebSocket column), §12 rows,
§13 items 5, 6, 16, 17, 23.

### R. RPC core, the socket links

| Files | Holds |
|---|---|
| `crates/ulo-rpc/Cargo.toml`, `src/lib.rs`, `src/transport.rs` | `Rpc`, `RpcCx`, `Reply`, `CallHeaders`, `LinkInfo`, `NoHandler` |
| `crates/ulo-rpc/src/frame.rs`, `src/codec.rs` | the grammar with its `t` literals, the JSON codec, the CBOR codec over `ciborium`, `PayloadKind` |
| `crates/ulo-rpc/src/link.rs` | `Link` with `prepare`, `listen`, `connect`, `drain`, `close`, `bound`; `Inbound`, `Outbound`, `Delivery`, `ReplyPath`, `Ack`, `Capabilities`, `DeliveryMode`, `Ordering` |
| `crates/ulo-rpc/src/server.rs` | `Server<L>`: `prepare` (`Link::prepare`, patterns, shapes against `Capabilities`, `PayloadKind` against `binary`, `max_inflight` zero through X22), `bind`, `serve`, `drain`, `close`, `bound` |
| `crates/ulo-rpc/src/dispatch.rs` | per-delivery execution, `deadline-ms`, `cancel`, the four shapes, `dispatch_late`, the envelope-or-payload split per `Capabilities` |
| `crates/ulo-rpc/src/client.rs`, `src/client_module.rs` | `RpcClient`, `RpcClientModule` with `.timeout(Bound)`, `RpcError`, the miss mapping under `miss_signal`, the duplicate-reply drop |
| `crates/ulo-rpc/src/extract.rs` | `Payload<T>`, `Inbound<T>`, `CallHeaders` as `FromCall<Rpc>` |
| `crates/ulo-rpc/src/__private.rs` | `RpcHandler`, the payload-kind probe |
| `crates/ulo-rpc-macros/Cargo.toml`, `src/lib.rs`, `src/message.rs`, `src/event.rs` | `#[message]`, `#[event]` |
| `crates/ulo-rpc-tcp/Cargo.toml`, `src/lib.rs`, `src/link.rs` | the TCP `Link`: the 4-byte length prefix under `max_frame`, `goaway`, `ulo_net::Tls`, `Addressed`, `bound` |
| `crates/ulo-rpc-udp/Cargo.toml`, `src/lib.rs`, `src/link.rs` | the UDP `Link`: one datagram per frame, unary and events, `Addressed`, `bound` |

Replaces on `master`: the RPC half of the old `ulo` core (`ulo::rpc`, `RpcClient`, `wire`).
Rewrites in place: `ulo-rpc-tcp`, `ulo-rpc-udp`. Brings back: both. Creates: `ulo-rpc`,
`ulo-rpc-macros`.

Design sections: §2.9 (`messaging.system`), §5.1–§5.4, §10 (RPC column), §12 rows, §13 items 11,
12, 15.

### B. The broker links and the RPC conformance suite

| Files | Holds |
|---|---|
| `crates/ulo-rpc-nats/Cargo.toml`, `src/lib.rs`, `src/link.rs` | queue-group subscribe under the default group, `.group(..)`, `_INBOX` replies, the payload as the body with NATS headers, no-responders, drain, `tls://` |
| `crates/ulo-rpc-redis/Cargo.toml`, `src/lib.rs`, `src/link.rs` | pub/sub per pattern carrying the whole frame, the receiver count, `FanOut`, `rediss://` |
| `crates/ulo-rpc-rabbitmq/Cargo.toml`, `src/lib.rs`, `src/link.rs` | queues, `reply_to` and `correlation_id`, confirm mode with `mandatory`, the per-consumer prefetch from `max_inflight`, `basic.reject`, `basic.cancel`, `amqps://` |
| `crates/ulo-rpc-mqtt/Cargo.toml`, `src/lib.rs`, `src/link.rs` | `$share/<group>/` subscribe under the default group, Response Topic and Correlation Data, PUBACK and PUBREC 0x10, the CONNACK limits and the shared-subscription check at `bind`, `mqtts://` |
| `crates/ulo-rpc-kafka/Cargo.toml`, `src/lib.rs`, `src/link.rs` | the consumer group, the reply topic, headers, pause and commit, handler topics created at `bind`, `security.protocol=SSL` |
| `crates/ulo-rpc-conformance/Cargo.toml`, `src/lib.rs`, `src/cases/*.rs` | `Broker`, `Budget`, the scenario list of §5.2, the stamping macro |

Depends on R's `Link` contract. Rewrites in place: all six. Brings back: all six. Creates nothing.

Design sections: §5.2, §5.3, §10 (RPC column), §13 items 12, 13, 14.

### G. gRPC

| Files | Holds |
|---|---|
| `crates/ulo-grpc/Cargo.toml`, `src/lib.rs`, `src/transport.rs` | `Grpc`, `GrpcCx`, `Reply`, `GrpcMetadata`, `PeerAddr`, `include_proto!`, the `HttpCarried` impl |
| `crates/ulo-grpc/src/method.rs` | `Method`, the `Shape` re-export |
| `crates/ulo-grpc/src/dispatch.rs` | the `:path` dispatcher over `tonic::server::Grpc` and the four shape-service traits, UNIMPLEMENTED off the table |
| `crates/ulo-grpc/src/server.rs` | `Server`: `http2` over `ulo-hyper-serve`, `grpc-timeout`, `max_inflight`, `max_per_connection`, `max_concurrent_streams`, `tls`, the zero refusals through X22, drain with the health switch |
| `crates/ulo-grpc/src/status.rs` | kind to code, `grpc-status-details-bin` through tonic-types and the hand-packed `Value`, the HTTP-status table |
| `crates/ulo-grpc/src/extract.rs` | `Message<T>`, `Request<T>`, `Streaming<T>`, `GrpcMetadata`, each body-consuming |
| `crates/ulo-grpc/src/pre_dispatch.rs` | `pub type PreDispatch = ulo_http::PreDispatch<Grpc>` and the gRPC server's run of the stage |
| `crates/ulo-grpc/src/health.rs`, `src/reflection.rs` | `GrpcHealth` over `HealthReporter`, reflection registration (`build_v1`, `build_v1alpha`) |
| `crates/ulo-grpc/src/client.rs` | `GrpcClientModule`, `GrpcEndpoint`, `ClientTls`, `outgoing` |
| `crates/ulo-grpc/src/__private.rs` | `GrpcHandler` |
| `crates/ulo-grpc-macros/Cargo.toml`, `src/lib.rs`, `src/method.rs` | `#[method(Marker)]` |
| `crates/ulo-build/Cargo.toml`, `src/lib.rs`, `src/markers.rs` | `configure().compile(..)` over tonic-prost-build and the vendored `protoc` (feature `vendored-protoc`, default on, yielding to `PROTOC`), the `ServiceGenerator` writing the `Method` markers, `file_descriptor_set_path` |

Rewrites in place: `ulo-grpc`, `ulo-build`. Brings back: both. Creates: `ulo-grpc-macros`.

Design sections: §3.4 (X23 as consumed), §6, §10 (gRPC column), §12 rows, §13 items 7, 18, 19,
20.

### Q. GraphQL

| Files | Holds |
|---|---|
| `crates/ulo-graphql/Cargo.toml`, `src/lib.rs` | `Engine` with `playground_html`, `GqlRequest`, `GqlResponse` with `Outcome`, `GqlError` |
| `crates/ulo-graphql-http/Cargo.toml`, `src/lib.rs`, `src/controller.rs`, `src/module.rs`, `src/playground.rs` | the controller, `GraphqlModule`, `GraphqlConfig` with `.engine::<Q>()`, the GraphQL-over-HTTP status rules from `Outcome`, the `Accept` negotiation, the playground |
| `crates/ulo-graphql-ws/Cargo.toml`, `src/lib.rs`, `src/gateway.rs` | the graphql-transport-ws `Gateway` over `Engine::subscribe`, its close codes, `connection_init_timeout` |
| `crates/ulo-graphql-async-graphql/Cargo.toml`, `src/lib.rs` | `AsyncGraphql<S, C>`, `dep` |
| `crates/ulo-graphql-juniper/Cargo.toml`, `src/lib.rs` | `Juniper<Q, M, Sub>` over `execute` and `resolve_into_stream`, `dep` |

Depends on W's `Gateway` contract and on `ulo-http`'s controller API. Rewrites in place:
`ulo-graphql-async-graphql`, `ulo-graphql-juniper`. Brings back: both. Creates: `ulo-graphql`,
`ulo-graphql-http`, `ulo-graphql-ws`.

Design sections: §7, §12 rows, §13 items 7, 21.

### E. The five embedding adapters and the HTTP conformance suite

| Files | Holds |
|---|---|
| `crates/ulo-http-axum/Cargo.toml`, `src/lib.rs`, `src/layer.rs`, `src/run.rs` | `Axum`, the `ConnInfo` and `OriginalPath` layer, the `NestedPath` check, `run` over `axum::serve::Serve` |
| `crates/ulo-http-salvo/Cargo.toml`, `src/lib.rs`, `src/handler.rs`, `src/run.rs` | `Salvo`, `SalvoRequest<'r>`, the `Handler`, the prefix strip, `run` over `serve_with_graceful_shutdown` |
| `crates/ulo-http-poem/Cargo.toml`, `src/lib.rs`, `src/endpoint.rs`, `src/run.rs` | `Poem`, the `Endpoint`, `take_upgrade`, `run` over `run_with_graceful_shutdown` |
| `crates/ulo-http-actix/Cargo.toml`, `src/lib.rs`, `src/service.rs`, `src/pump.rs`, `src/run.rs` | `Actix`, `scope`, the payload pump, the connection-type flag, the prefix strip, `run` with `disable_signals`, `shutdown_timeout` rounded up and `ServerHandle::stop(true)` |
| `crates/ulo-http-rocket/Cargo.toml`, `src/lib.rs`, `src/handler.rs`, `src/convert.rs`, `src/upgrade.rs`, `src/run.rs` | `Rocket`, the catch-all routes, the http 0.2 conversion, the body buffered under `body_limit`, the prefix strip, `Forward`, `IoHandler`, the local-cache `Routing`, `run` with `shutdown.ctrlc = false` and `grace` rounded up |
| `crates/ulo-http-conformance/Cargo.toml`, `src/lib.rs`, `src/cases/*.rs` | `Host`, the hyper reference host, the scenario list of §3.8, the rocket test fairing for `Routing`, the stamping macro |

Rewrites in place: `ulo-http-salvo`, `ulo-http-poem`, `ulo-http-actix`, `ulo-http-rocket`.
Recreates: `ulo-http-axum`, which `master` has as the old adapter and the branch does not have at
all. Brings back: the four rewritten crates. Creates: `ulo-http-axum`, `ulo-http-conformance`.

Design sections: §3.8, §10 (embedded column), §12 rows, §13 items 9, 10.

### D. The development command

| Files | Holds |
|---|---|
| `crates/ulo-cli/Cargo.toml`, `src/main.rs` | the hidden `__exec` subcommand; the `dev` feature as on `master` |
| `crates/ulo-cli/src/commands/dev.rs` | watch, rebuild, restart; `--listen` binding the held socket and passing it as descriptor 3 with `LISTEN_FDS`, `LISTEN_FDNAMES` naming the address, `ULO_DEV=1`; `place_listen_fd` carried over; the UDP refusal |
| `crates/ulo-cli/src/commands/exec.rs` | the `ulo __exec -- <child> <args>` trampoline: `LISTEN_PID` set to its own pid, then `exec` |
| `crates/ulo-net/src/endpoint.rs` | the `ULO_DEV` resolution in `EndpointSpec::resolve`: an `Addr` whose text equals an inherited socket's name resolves to that socket |

Rewrites in place: `ulo-cli`'s `commands/dev.rs`; `commands/new.rs`, `commands/generate.rs` and
`templates/` are untouched in the race and target the old API, which D's log records as owed.
Brings back: `ulo-cli`. Creates nothing.

Design sections: §2.7, §9, §12 rows, §13 item 8.

### Not in 2b

`ulo-config`, the six `ulo-db-*` crates, `ulo-health`, `integration-tests` and `examples` stay
excluded.

## Cross-area contracts

Each row is an item one area calls in another; the owner keeps it as the spine writes it.

### Owned by S

| Item | Called by | For |
|---|---|---|
| `Inputs::also_seeded_by::<U>()`, the merge rule | W | `SessionHandle`, `ConnectionInfo`, `UpgradeHead` under `Ws` and `WsConnect` |
| `AppHandle::mounted::<T>()`, `Mounted::handlers_of::<U>()` | W, Q | the hand-off reading `MountedHandler<Ws>` and `<WsConnect>`; the standalone server pairing the two |
| `TransportMetadata`, `__private::{MetaProbe, MetaMismatch}`, the probe `emit::metadata` writes | W, R, G (their attributes through `ulo-handler-codegen`) | every transport's `#[meta]` check |
| `UpgradeHandler::{paths, upgrade, prepare, bound, drain, close}`, `Upgrades::register` | W | the gateway hand-off on the HTTP port and on an embedding |
| `ulo_transport::prepare::{Failures, Failure, Names, zero_bound, zero_count}` | W, R, G, Q, E | every `prepare` failure |
| `PreDispatch<T: HttpCarried = Http>`, `HttpCarried` | G | the gRPC stage |
| `ulo_hyper_serve::{Serve, ServeConfig, Accepted, Io}` | W (the standalone server), G (the HTTP/2 server) | the accept loop; the hyper backend consumes it inside S |
| `Request`, `Response`, `HttpBody`, `ConnInfo`, `OnUpgrade`, `Upgraded`, `Embed`, `Embedded`, `Handle`, `Service::respond`, `Routing`, `Forwardable`, `OriginalPath`, `EmbedLimits`, `RequestBody`, `Disconnect`, `Miss` | E | as built in 2a, unchanged |
| `ModuleDef::controller::<C>().at(prefix)`, `PreDispatch`, `Host<T>`, `Dep<RequestHead>` | Q | the GraphQL controller's path and its reads |
| `ulo_net::{EndpointSpec, Endpoint, Activation, Tls, BoundListener}` | R (TCP, UDP), W, G, D | endpoints, inherited sockets, TLS |

### Owned by W

| Item | Called by | For |
|---|---|---|
| `Gateway`, `GatewayConfig`, `Session<T>`, `ConnectRefused`, `Frame`, `Reply`, `WsModule` | Q | the graphql-transport-ws gateway |
| the `subprotocols` setting and its echo | Q | `graphql-transport-ws` in the 101 |

### Owned by R

| Item | Called by | For |
|---|---|---|
| `Link`, `Inbound`, `Outbound`, `Delivery`, `ReplyPath`, `Ack`, `Capabilities`, `DeliveryMode`, `Ordering`, `Frame`, `PayloadKind`, the codec trait | B | the seven links and the conformance suite |
| `RpcClient`, `RpcClientModule`, `RpcError` | B (the suite) | the caller's side of every scenario |

### Owned by G

| Item | Called by | For |
|---|---|---|
| `Method` | the build step's generated markers (`ulo-build`, inside G) | `PATH`, `SHAPE`, `Request`, `Response` |

### Owned by E

Nothing outward: every adapter consumes S's embedding surface and exports its aliases.

### Owned by D

Nothing outward. `ulo-net`'s `EndpointSpec::resolve` is D's one file outside `ulo-cli`, and its
signature is frozen; the `ULO_DEV` read is added inside it.

## Points each area resolves in its own log

Not settled by the designs or by the spine. The owning area decides and logs the decision.

- **S.** How `HttpCarried` is sealed while `ulo-grpc` implements it for `Grpc`; whether
  `Accepted` carries the `Draining` signal or the closure receives it apart; the `JoinSet`
  shutdown order on `close`.
- **W.** Whether a queued control frame reaches the wire without an explicit poll of the read
  side after a Close, and where the loop gives it one; the `DisconnectReason` for a tungstenite
  `Error::Capacity`; how a same-port gateway's defaults are read from `WsModule` when the HTTP
  server's `prepare` runs.
- **R.** The `credit` frame's exact fields, reserved and unsent; the TCP `max_frame` default; how
  the Redis whole-frame carriage shares code with TCP's.
- **B.** The names of the Kafka topic-creation settings (partitions and replication factor); how
  the CONNACK shared-subscription check reaches `bind`'s error from rumqttc's event loop; whether
  `UNKNOWN_TOPIC_OR_PARTITION` surfaces on a produce with auto-create off, which the design states
  and the build confirms.
- **G.** Whether `tonic::server::Grpc` and the four shape-service traits suffice to dispatch
  without the generated trait, which the design states from tonic's generated code and the build
  confirms; how the dispatcher recovers `grpc-timeout` once `fw-hyper-serve` hands it a bare
  `http::Request`.
- **Q.** Whether an absent `Accept` is read as the legacy `application/json`, as §7 states, or the
  specification's watershed date changes it; how `GraphqlConfig::engine::<Q>()` threads the
  qualifier into the controller's `Dep<dyn Engine, Q>`.
- **E.** The per-host `run` type parameters beyond the shapes §3.8 gives; the host-by-host moment a
  dropped response body is observed on poem and rocket, which the declared `AtNextWrite` states
  and the suite checks; whether rocket's `Shutdown` handle is obtainable before `launch()` through
  `Rocket<Ignite>`.
- **D.** What `--listen` accepts beyond `host:port` (the bare-port shorthand `master` had); how
  `__exec` stays out of `--help`; whether `watchexec` is kept over bare `notify`.

## After the race

The coordinator compiles the workspace, writes the per-crate `tests/conformance.rs` for the seven
links and the five adapters, folds the eight logs into `transports/DIVERGENCES.md`, and the design
author answers them in `transports/RESPONSE.md`.
