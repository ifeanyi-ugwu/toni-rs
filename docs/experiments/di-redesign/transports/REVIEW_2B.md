# Race 2b readiness: §3.8 adapters, §4–§9

Reviewed: `transports/DESIGN.md` §3.8 and §4–§9 against race 2a as built on `experiment/di-redesign`
at `230d9430` (`crates/ulo`, `crates/ulo-transport`, `crates/ulo-net`, `crates/ulo-tokio`,
`crates/ulo-http`, `crates/ulo-http-hyper`), the fourth through nineteenth responses in
`RESPONSE.md`, `EMBEDDING_REVIEW.md`, the "Not covered" lists of `divergences/batch2a-embed.md` and
`divergences/batch2a-followups.md`, the library sources in the local registry, and the old
implementation on `master` as evidence of what each library allows. The direction is taken as
given; every item is a refinement inside it or a question for the author.

Each claim is marked **Read** (`file:line` or a registry path), **Spec** (the governing
specification), **Probed** (compiled or run), or **Unverified**. No cargo was run in the ulo
workspace, and no new probe crate was needed: every claim below is Read or Spec, and the items a
build would have to show are listed under "Not verified here" at the end. Registry paths are under
`~/.cargo/registry/src/*/` at the versions named: tungstenite and tokio-tungstenite 0.28.0,
async-nats 0.46.0, redis 1.3.0, lapin 4.10.0, rumqttc 0.25.1, rdkafka 0.39.0, tonic 0.14.6 with
tonic-health, tonic-reflection, tonic-types and tonic-prost-build 0.14.6, prost-build 0.14.4,
async-graphql 7.2.1, juniper 0.16.2, axum 0.8.9, salvo 0.92.2, poem 3.1.12, actix-web 4.15.0, rocket
0.5.1, watchexec 8.2.0.

Refinements are `R1..`, each a concrete change with its reason; questions are `Q1..`, each one the
author decides. Both number across the whole document and are grouped by section.

## What holds

- The core's `Server` SPI as built carries what §4–§6 need: `prepare` with a default, `bind`,
  `serve`, `drain(DrainToken)`, `close`, and `bound()` defaulting to none for a socketless
  transport (Read, `crates/ulo/src/transport/server.rs:24-69`). `listen()` runs every `prepare`
  before any `bind` and collects `ConfigureErrors` (Read, `crates/ulo/src/app/mod.rs:199-236`).
- `Count` lives in `ulo-transport` with `Default | Max(u32) | Unlimited` and no conversion (Read,
  `crates/ulo-transport/src/count.rs`), as the sixteenth response placed it. `Admission::new`
  takes `Option<usize>` and the HTTP server converts at that boundary (Read,
  `crates/ulo-http/src/backend.rs`, `inflight_limit`). WebSocket, RPC and gRPC can take the same
  type and the same conversion.
- `PrepareError` exists in the core with `names` and a text closure, and `ConfigureErrors::collect`
  runs one `TypeName::colliding` pass over every failing transport and every `PrepareError`'s names
  (Read, `crates/ulo/src/error/configure.rs`). A 2b transport answering a `PrepareError` from
  `prepare` joins that pass with no further core change.
- `Mount::once::<K>` (X15) exists, keyed by `TypeId` within one controller's mount, with the
  gateway's connect handler as its documented use (Read, `crates/ulo/src/transport/controller.rs`,
  `Mount::once`). `emit::mount_param_ident()` names the mount function's `&mut Mount<'_>` for a
  WebSocket handler value to call it (Read, `crates/ulo-handler-codegen/src/emit.rs`,
  `mount_param_ident`).
- `Shape` is in the core with the four call shapes (Read, `crates/ulo/src/transport/handler.rs`,
  `Shape`), and `MountFn.shape` takes a transport's `.shape(..)` argument (Read, `emit.rs`,
  `MountFn`). RPC and gRPC attributes record their shape there.
- `CancelReason` has every variant §2.6 names, `on_stream_end` and `report_stream_end` exist, and
  `Tracked<S>` reports `Completed` or `CutOff(reason)` on drop (Read,
  `crates/ulo/src/execution/mod.rs:76-104`, `crates/ulo-transport/src/tracked.rs`). `dispatch_late`
  and `recover(None, ..)` exist for the mid-stream and the no-handler paths (Read,
  `crates/ulo/src/transport/pipeline.rs`, `dispatch_late`, `recover`).
- `Execution::open_terminal(&DrainToken, ..)` exists for a connection's cleanup during the drain
  (Read, `crates/ulo/src/execution/mod.rs:153`), which `OnDisconnect` needs.
- The upgrade hand-off point exists: `ulo_http::UpgradeHandler { paths(&self, &AppHandle),
  upgrade(&self, Request) -> BoxFuture<'static, Response> }` and the `Upgrades` module metadata,
  read by `prepare_app` and matched before routing on any request carrying `Upgrade` (Read,
  `crates/ulo-http/src/upgrade.rs`, `crates/ulo-http/src/server.rs`, `upgrade_paths`,
  `crates/ulo-http/src/service.rs`, `route`).
- The embedding surface is built to the ninth and tenth responses: `Embed { NAME, HostRequest<'r>,
  limits() }`, `Embedded::forward`, `Service::respond(host, req)`, the tower impl for
  `HostRequest<'r> = Parts`, `Handle::{service, host, stopping, app}`, `OriginalPath`,
  `Forwardable`, `Miss`, and the seven-field `EmbedLimits` with `RequestBody` and `Disconnect`
  enums (Read, `crates/ulo-http/src/embed.rs`). `Routing` has the five variants of the seventh
  response (Read, `crates/ulo-http/src/routing.rs`).
- §8 is built: `ulo_tokio::Timer`, `shutdown_signal()` resolving to `Signal::new("SIGTERM")` and
  kin, `spawn` and `spawn_in` returning `JoinHandle<Option<T>>` (Read,
  `crates/ulo-tokio/src/lib.rs`). Nothing in §8 remains for race 2b beyond what §9 consumes.
- `ulo-net`'s `Activation` requires `LISTEN_PID` to equal the process id (T11, Read,
  `transports/DIVERGENCES.md:167-178`), which is the contract §9's trampoline exists to meet.
- The library APIs §4–§7 rely on exist at the registry versions: tungstenite's server-role
  constructor and `derive_accept_key` (Read, `tokio-tungstenite-0.28.0/src/lib.rs:137-164`,
  `tungstenite-0.28.0/src/handshake/mod.rs:117`); async-nats `drain`, `request` with
  `RequestErrorKind::NoResponders`, `max_payload`, `queue_subscribe` (Read,
  `async-nats-0.46.0/src/client.rs:322, 554, 675, 746`); redis `publish -> usize` and the async
  `PubSub` (Read, `redis-1.3.0/src/commands/mod.rs:1608`, `src/aio/pubsub.rs:376-491`); lapin
  `wait_for_confirms -> Vec<BasicReturnMessage>`, `basic_cancel`, `basic_nack` (Read,
  `lapin-4.10.0/src/channel.rs:324, 624, 697`); rumqttc v5 `ConnAckProperties::max_packet_size`
  and `receive_maximum`, `PubAckReason::NoMatchingSubscribers` (Read,
  `rumqttc-0.25.1/src/v5/mqttbytes/v5/connack.rs:104-111`, `puback.rs:8`); rdkafka `pause`,
  `resume`, `commit`, `store_offset` (Read, `rdkafka-0.39.0/src/consumer/mod.rs:283-404`); tonic
  `Status::with_details`, `GRPC_STATUS_DETAILS`, the `GrpcTimeout` service, tonic-health
  `set_service_status`, tonic-reflection `build_v1`/`build_v1alpha`, tonic-types `ErrorDetails`
  with `set_bad_request`, `set_retry_info`, `set_error_info`, `set_help` (Read,
  `tonic-0.14.6/src/status.rs:574, 622`, `src/transport/service/grpc_timeout.rs:14, 103`,
  `tonic-health-0.14.6/src/server.rs:74`, `tonic-reflection-0.14.6/src/server/mod.rs:77, 95`,
  `tonic-types-0.14.6/src/richer_error/error_details/mod.rs:447-805`); async-graphql `execute`,
  `execute_stream`, `Request::data`, `sdl`, `GraphiQLSource`, `ALL_WEBSOCKET_PROTOCOLS` (Read,
  `async-graphql-7.2.1/src/schema.rs:451-658`, `src/http/mod.rs:25-33`); juniper `execute`,
  `resolve_into_stream`, `as_sdl`, `graphiql_source` (Read, `juniper-0.16.2/src/lib.rs:193, 244`,
  `src/schema/model.rs:234`, `src/http/graphiql.rs:11`).

## §3.8 The five embedding adapters

**R1. Write each adapter's `EmbedLimits` and `HostRequest<'r>` into §3.8, as a table.** (Read;
Unverified where marked.) The design states each limit's variation in prose and leaves a build
agent to derive five structs. The table a build needs, from the sources:

| Adapter | `HostRequest<'r>` | `peer_addr` | `upgrades` | `forward_miss` | `host_extensions` | `tls_info` | `request_body` | `disconnect` |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| axum | `http::request::Parts` (Read, embed.rs:71, the tower bound) | `false` unless `.peer_addr(true)` (Probed in EMBEDDING_REVIEW e01) | `true` (hyper `OnUpgrade` in extensions, Read embed.rs:829) | `false` | `true` | `false` | `Streamed` | `AtClose` (hyper) |
| salvo | see Q1 | `true` (`Request::remote_addr`, Read EMBEDDING_REVIEW R8) | `true` (`OnUpgrade` left in extensions, Read R8) | `false` | `true` (`Request::extensions`, Read `salvo_core-0.92.2/src/http/request.rs:608`), with Q1's caveat | `false` | `Streamed` | `AtClose` (hyper 1) |
| poem | `poem::Request` (owned; the lifetime ignored) | `true` (`remote_addr`, Read R8) | `true` (`take_upgrade`, Read R8) | `false` | `true` (`Request::extensions`, Read `poem-3.1.12/src/request.rs:392`) | `false` | `Streamed` | `AtNextWrite` (Unverified; the old adapter's F102 record) |
| actix-web | `actix_web::HttpRequest` (Read embed.rs:69) | `true` (`peer_addr`, `connection_info`, Read `actix-web-4.15.0/src/request.rs:401, 412`) | `false` (tenth decision, Read RESPONSE.md:599) | `false` | `false` | `false` | `Streamed`, through the worker-local pump | `AtNextWrite` (the pump's, Read EMBEDDING_REVIEW R15) |
| rocket | `rocket::Request<'r>` (Read embed.rs:68, RESPONSE.md:755) | `true` (`remote`, `client_ip`, Read `rocket-0.5.1/src/request/request.rs:333, 430`) | `true` (`IoHandler`, Read EMBEDDING_REVIEW R17) | `true` | `false` | `false` | `Buffered(cap)`, Q3 | `AtNextWrite` (Unverified, F102) |

`Disconnect::AtNextWrite` on poem and rocket rests on the old adapters' behaviour and has not been
observed on the new ones; the conformance suite's cancellation scenario is where it gets checked,
and the limits table "checked in the other direction" (§3.8) catches a wrong declaration either
way. The table belongs in the design so the suite and the adapters are written against one record.

**R2. State that four adapters strip the `.nested_at` prefix themselves.** (Read.) §3.8 opens
"A host strips the nest prefix before the app sees the request". axum's `nest_service` strips
(Probed, EMBEDDING_REVIEW e01) and poem's `Route::nest` strips (Read,
`poem-3.1.12/src/route/router.rs:178-184`, "strip the prefix"). salvo matches a path through its
router and hands the handler the full `uri().path()` (Read, `salvo_core-0.92.2/src/routing/router.rs`
has no strip or nest, `src/http/request.rs:324`); actix's `web::scope` leaves `req.path()`
whole and puts the rest in `match_info` (Read, `actix-web-4.15.0/src/request.rs:162, 186`); rocket's
`mount` leaves `req.uri()` whole with the tail in a `<path..>` segment. On those three the adapter
strips `.nested_at` before building the app's `Request`, and a request not under the prefix is the
host's misconfiguration: answer it as the app's 404 with `Routing::NotFound` and log at `warn`
once, as the axum adapter logs a `NestedPath` mismatch. Without this sentence a build agent on
salvo passes the full path and every route misses.

**R3. Give each `run` helper its signature and its clock conversion.** (Read.) `Handle::host`
takes any `Future<Output = Result<(), E>> + Send + 'static` (Read, embed.rs:712-716), and
`Handle::app()` answers the `AppHandle` whose `drain_timeout()` is the host's stop bound once
`prepare` has run (Read, embed.rs:749-751). The host entry points differ per crate, and the helper
must take the host object *before* its terminal call so it can attach the stop signal and the
bound:

- axum: `run(app, &handle, serve: axum::serve::Serve<L, M, S>, signal)`, attaching
  `with_graceful_shutdown(handle.stopping())` itself (§3.8 already states this).
- salvo: `run(app, &handle, server: salvo::Server<A>, service: salvo::Service, signal)`, calling
  `serve_with_graceful_shutdown(service, handle.stopping(), Some(drain))` (Read,
  `salvo_core-0.92.2/src/server.rs:90, 186`).
- poem: `run(app, &handle, server: poem::Server<L, A>, endpoint, signal)`, calling
  `run_with_graceful_shutdown(endpoint, handle.stopping(), Some(drain))` (Read,
  `poem-3.1.12/src/server.rs:149`).
- actix-web: `run(app, &handle, server: HttpServer<F, I, S, B>, signal)`, calling
  `.disable_signals().shutdown_timeout(secs).run()` and installing a future that awaits
  `handle.stopping()`, calls `ServerHandle::stop(true)`, then awaits the server (Read,
  `actix-web-4.15.0/src/server.rs:388, 430`). `shutdown_timeout` takes whole seconds as `u64`, so
  the window is rounded up.
- rocket: `run(app, &handle, rocket: Rocket<Build>, signal)`, overriding
  `shutdown.ctrlc = false` and `shutdown.grace` before `ignite()`, taking the `Shutdown` handle
  and installing a future that notifies it on `stopping()` then awaits `launch()`. `grace` and
  `mercy` are `u32` seconds (Read, `rocket-0.5.1/src/config/shutdown.rs:212-229`), so the window is
  rounded up there too.

The whole-second rounding on actix and rocket is worth one sentence in §3.8's drain paragraph,
since the design says "one clock, set once" and the host's copy of it differs by up to a second.

**R4. The embedding conformance suite needs a crate, a `Host` trait and its scenario list.**
(Read.) §3.8's last paragraph describes what the suite asserts; `divergences/batch2a-embed.md:276`
lists it as not built. The RPC side already has the form: a `Broker` trait with `start`, the two
halves under test, `disrupt` and a `Budget`, and one macro stamping every case per implementation
(Read, `git show master:crates/ulo-rpc-conformance/src/lib.rs`, lines 54-110, 603-625). Mirror it:
`ulo-http-conformance` with `trait Host { fn start(app_service, mode: Nested | Fallback) -> Self;
fn base_url(&self) -> String; fn limits() -> EmbedLimits; fn stop(self) }`, the hyper backend as
the reference `Host`, and the cases: routing hit, 404 with `NoRoute`, 405 with `Allow`, `OPTIONS`
204, `HEAD` from `GET`, each extraction failure's status (400, 413, 415, 422) with the problem
document, a reshaped error, an unscoped entry answering a preflight, SSE with an `error` event, a
disconnect mid-stream firing `Disconnected` at the declared `Disconnect` moment, a 101 upgrade and
one frame echoed where `upgrades`, `Host<T>` present and absent (500) where `host_extensions`, a
`forward` copy on the two `false` hosts, `Routing` in the response extensions where observable
(poem and salvo responses have extensions, Read `poem-3.1.12/src/response.rs:210`; rocket's is read
from the local cache by a fairing), the 503 before `listen()` and after `close`, and the drain in
both HTTP/1.1 and HTTP/2 shapes. The limits table is asserted in both directions: a scenario a host
declares unsupported must fail on it.

**Q1. salvo's host store is `Depot`, which is not the request.** salvo middleware conventionally
stores values in a `Depot` passed beside the request, and the request's own `extensions()` is a
separate map (Read, `salvo_core-0.92.2/src/depot.rs:38`, `src/http/request.rs:608`). Does salvo
declare `host_extensions: true` reading `Request::extensions` only, or name
`type HostRequest<'r> = (&'r salvo::Request, &'r salvo::Depot)` so a `forward` copy can read the
`Depot`? The GAT admits the tuple. A value a salvo middleware put in the `Depot` is unreachable
under the first choice.

**Q2. Where does `Routing` go on rocket, and does the suite assert it there?** The seventh response
puts it in the request's local cache for a fairing's `on_response` (Read, RESPONSE.md:595). The
conformance paragraph says `Routing` is asserted "on the hosts where a test layer can observe
response extensions". Is rocket's cache slot asserted through a test fairing, or is rocket exempt?

**Q3. The rocket body cap.** `RequestBody::Buffered(cap)` is declared without a number; the old
adapter used 32 MiB (Read, `git show master:crates/ulo-http-rocket/src/rocket_adapter.rs`, line
36). Is 32 MiB the declaration, and is it a builder setting on `ulo_http_rocket::Embedded` or a
constant?

## §4 WebSocket

**R5. A gateway on the HTTP server's port has no lifecycle, and the design needs to give it one.**
(Read.) Three facts about the built code decide this. First, the typed handlers of a transport reach
code only through `Mounted<'_, T>` inside a `Server<Transport = T>`'s `prepare` and `bind`
(`mounted_parts` is private, Read `crates/ulo/src/transport/server.rs`, `mounted_parts`,
`ErasedServer::prepare`); `AppHandle::handlers()` answers `HandlerInfo` alone, with no module, no
enhancer tiers and no handler value (Read, `crates/ulo/src/app/handle.rs:109`). Second,
`UpgradeHandler` has `paths(&self, &AppHandle)` and `upgrade(&self, Request)` and nothing else
(Read, `crates/ulo-http/src/upgrade.rs:19-27`). Third, `ulo_http::Server::drain` calls
`backend.drain()` and `close` calls `backend.close()`, and the hyper backend's drain covers the
connections hyper still owns; an upgraded connection has left hyper (Read,
`crates/ulo-http/src/server.rs`, `impl ulo::Server for Server<B>`). So a same-port gateway today
cannot find its `MountedHandler<Ws>` values, receives no `DrainToken` for `open_terminal`, is never
told to close idle connections with 1001, and has no moment for `AfterInit`. §4.1's "Gateways are
identical on either [port]" and §10's WebSocket column are unmet for the same-port case.

Change, two additive pieces:

- Core (X19): `AppHandle::mounted::<T: Transport>() -> Result<Vec<MountedHandler<T>>, TimerMissing>`,
  which is `mounted_parts` made public, or `Mounted<'_, T>::handlers_of::<U: Transport>()`. The
  first serves the `UpgradeHandler`, which holds an `AppHandle`; the second serves a transport whose
  server covers two markers (R6).
- `ulo-http` (X20): `UpgradeHandler` gains four defaulted methods mirroring `Server`:
  `fn prepare(&self, app: &AppHandle) -> Result<(), BoxError>` whose failures join the HTTP
  `PrepareError` (duplicate gateway paths, a gateway's own limits), `fn bound(&self, app:
  &AppHandle)` after `bind` (where `AfterInit` runs), `fn drain(&self, token: DrainToken) ->
  BoxFuture<'_, ()>` and `fn close(&self) -> BoxFuture<'_, ()>`. `Server<B>::drain` and `close`
  call every registered handler's beside the backend's, concurrently; `Embedded<A>` does the same,
  since a host with `upgrades: true` has the same connections.

The alternative is a socketless `fw_ws::Server` the user binds beside the HTTP one for same-port
gateways; that puts a `main` step where the design promised none and still needs X19 for the two
markers. The `UpgradeHandler` extension keeps the WebSocket-on-HTTP path module-driven (Q4).

**R6. `Ws` and `WsConnect` are two transports, and a `Server` has one.** (Read.) `Server::Transport`
is one associated type, and `ErasedServer::prepare` builds `Mounted<S::Transport>` from the graph
filtered by that one marker (Read, `crates/ulo/src/transport/server.rs`, `ErasedServer`,
`mounted_parts`). The standalone `fw_ws::Server` with `Transport = Ws` therefore never receives the
connect handlers mounted under `WsConnect` through `Mount::once`. X19's `handlers_of::<U>()` on
`Mounted` closes it: `fw_ws::Server::prepare` reads `mounted.handlers()` for the message handlers
and `mounted.handlers_of::<WsConnect>()` for the connect handlers, pairing them by controller key.
Without it the design's two-transport split cannot be bound.

**R7. F308 is unresolved in the code, and the design's sentence does not match the API.** (Read.)
`declare_transport_inputs` records one seeder per declaration (`seeders: vec![..]`,
`crates/ulo/src/graph/wire.rs:370, 674-689`) and step 2 reports a second transport's declaration
of the same key as `InputConflict` (`wire.rs:866-884`). The module-side `Input::seeded_by(self)`
consumes the builder and returns `()` (Read, `crates/ulo/src/binding/alias.rs:66`), so "`seeded_by`
once per seeder" in core DESIGN §6.4 and transports §2.10 has no spelling. Change, on the transport
side only, which is where WebSocket declares: `Inputs::input::<T>()` keeps its shape and gains
`fn also_seeded_by<U: Transport>(&mut self) -> &mut Self`, applying to the last `input` written.
`Ws::inputs` writes `d.input::<SessionHandle>().also_seeded_by::<WsConnect>()` and `WsConnect::inputs`
writes the mirror. The freeze merges two declarations of one key whose seeder sets are equal into
one `InputDecl` with both seeders and keeps `InputConflict` for any other pair; the per-handler
input check passes a handler of any seeder, as §6.4 states. `InputOrigin::Transport` gains nothing:
each side keeps its own origin, and the report names the first declaration. This is the "set of
seeders" option T15 recommended, placed on `Inputs` because that is the API the two markers write.

**R8. `#[guards(ws_connect = ..)]` on the impl is refused by X1; say so.** (Read.) X1 asserts every
controller-level scoped key against the `__ULO_KEY_<name>` constants the method attributes emit
(Read, `crates/ulo-handler-codegen/src/protocol/mod.rs:301`, `keys.rs`). The connect handler is
mounted by `fw-ws` through `Mount::once`, not by a method, so no `__ULO_KEY_*` is `"ws_connect"`,
and an impl-level `ws_connect = Guard` fails the key assertion with "is not the key of any
handler's transport in this impl". That is correct, and §4.1 should state that `connect_guards(..)`
on the gateway attribute is the one spelling for connect guards; §12 gains the row.

**R9. `Dep<Rooms, ChatGateway>` names a qualified binding nothing registers.** (Read.) A `Dep<T, Q>`
resolves a binding under `Key::of::<T, Q>()`; only a module's `ModuleDef` pushes bindings (Read,
`crates/ulo/src/binding/alias.rs`, `crates/ulo/src/module/def.rs:357`), and `Mount` records
handlers alone (Read, `controller.rs`, `Mount::handler`). A gateway's mount cannot bind `Rooms @
ChatGateway`, and the user's module does not know to. Change: `Rooms` is one binding from the
WebSocket module (Q4) and a gateway is addressed at runtime, `rooms.gateway::<ChatGateway>()` or
`rooms.namespace("lobby")`, answering a `RoomsIn` handle with the same `to_all`/`to_room`/
`to_client`/`except` methods. The example in §4.3 changes one line.

**R10. The broadcast adapter cannot be "replaced by importing a module".** (Read.) Two bindings
under one key are `DuplicateBinding` at `wire()` (core DESIGN §10.1 step 3), and override exists
in `TestApp` alone (core §11). `WsRedisModule::for_root(url)` beside the in-memory default is two
sources for `dyn BroadcastAdapter`. Change: the WebSocket module takes the adapter as
configuration, `fw_ws::WsModule::for_root()` binding the in-memory adapter and
`fw_ws::WsModule::for_root().broadcast(fw_ws_redis::Redis::url("redis://.."))` binding Redis's;
`fw-ws-redis` ships the adapter value, not a module. Membership "stays local to each process"
(§4.3) then holds by construction, and the old `ulo-ws-redis`'s Redis-set membership (Read, `git
show master:crates/ulo-ws-redis/src/service.rs`) is not carried over, which §4.3 already decides.

**R11. The envelope leaves five wire details to the build.** (Read; Spec.) `id` has no type:
graphql-transport-ws ids are strings (Spec, graphql-ws PROTOCOL.md, `id: ID`), the example writes
`7`. Define `id` as any JSON scalar, echoed byte-for-byte and compared by equality. `cancel` is a
reserved event name, so a user event `"cancel"` must be refused at compile time by the message
attribute and at `prepare` for a prefix-joined one; `complete`, `error` and `data` are server-side
keys and need no reservation. A `data` key absent on a request means `Payload<T>` fails `Missing`
with `param: "data"`. A text frame that is not a JSON object answers the `error` envelope with
`bad_request` and no `id`. Binary frames under `codec = msgpack` need a MessagePack crate, and none
is in the local registry (Read, `ls ~/.cargo/registry/src/*/ | grep -iE 'rmp|msgpack'` answers
nothing); see Q20.

**R12. Name the WebSocket limits with the settings vocabulary, and give the defaults.** (Read.)
§4.2 names "the size limit", "a gateway's connection limit", "the per-connection in-flight message
limit" and "each connection's outbound queue" without a spelling or a default. Following HTTP's
rule, a byte size is `*_limit: u64` and a count is `max_*: Count`: `message_limit` (bytes, one
message after reassembly; tungstenite's own defaults are 64 MiB per message and 16 MiB per frame,
Read `tungstenite-0.28.0/src/protocol/mod.rs:100-102`), `max_connections: Count` per gateway,
`max_inflight: Count` per connection, `max_outbound: Count` per connection, each settable on the
gateway attribute and defaulted on `fw_ws::Server`. `Count::Max(0)` on any is refused in `prepare`
with the hint the HTTP server writes (Read, `crates/ulo-http/src/server.rs`, `check_zero_counts`),
and a zero `message_limit` likewise. Over `message_limit` the connection closes with 1009 (Spec,
RFC 6455 §7.4.1), which tungstenite answers itself when configured through
`WebSocketConfig::max_message_size`.

**R13. Subprotocol negotiation is missing, and graphql-transport-ws cannot work without it.**
(Spec.) RFC 6455 §4.2.2 has the server select one subprotocol from the client's
`Sec-WebSocket-Protocol` list and echo it, or echo none; §4.1 has the client fail the connection
when the server names one it did not offer. graphql-transport-ws requires the subprotocol
`graphql-transport-ws` (Spec, PROTOCOL.md, "Communication"). The gateway attribute therefore needs
`subprotocols = ["graphql-transport-ws"]`: the handshake echoes the first offered name in that
list, and a client offering none of them gets a 101 without the header, after which the gateway
decides (graphql-ws's reference server closes 4406; a plain gateway proceeds). This runs at the
handshake, before connect guards, because the header is part of the 101.

**R14. Close-frame limits are the spec's, and `ConnectRefused::code` should enforce them where
built.** (Spec.) A close frame's reason is at most 123 bytes of UTF-8 (RFC 6455 §5.5.1); codes 1005,
1006 and 1015 are never sent in a frame (§7.4.1); 1004 is reserved; 1000–2999 are protocol codes
and 4000–4999 private use (§7.4.2); 1012 Service Restart and 1013 Try Again Later are IANA registry
entries, as §4.2 says. By §0.6, a fallible constructor with no infallible sibling returns `Result`
under the plain name: `ConnectRefused::code(code, reason) -> Result<Self, CloseCodeError>`, refusing
a reason over 123 bytes and a code in the forbidden set, as `EventId::new` refuses line
terminators. Two codes the list in §4.2 lacks: 1007 for a text frame that is not UTF-8 (§7.4.1,
which tungstenite raises as `Error::Utf8`) and 1010 for a mandatory extension the server did not
negotiate (client-only). Control frames are at most 125 bytes and never fragmented (§5.5), a Pong
carries the Ping's payload (§5.5.3), and after sending Close no data frame follows (§5.5.1);
tungstenite does all three and raises the UTF-8 case as `Error::Utf8` (Read,
`tungstenite-0.28.0/src/protocol/mod.rs`, `src/error.rs:52-61`), which is what §4.2's
"answered by the protocol layer" relies on and can say.

**R15. permessage-deflate is a documented limit.** (Read; Spec.) tungstenite 0.28.0 has no RFC 7692
support (Read: `deflate` appears only in `src/handshake/headers.rs`, a header name list). A client
offering `Sec-WebSocket-Extensions: permessage-deflate` is answered without the header, which RFC
6455 §9.1 allows. §4.2 should state it so the conformance suite and the documentation carry the
same sentence.

**R16. Name `Ws::Reply` and the item-error form.** (Read.) `Transport::Reply: Send` is the type an
interceptor's `next.run()` answers (Read, `crates/ulo/src/transport/mod.rs:69`), so it needs a
name: `fw_ws::Reply`, an enum `None | One(Frame) | Many(Tracked<BoxStream<'static, Result<Frame,
BoxError>>>)` as the HTTP `Response` holds a `Tracked` body for a stream. `IntoReply<Ws>` is
implemented for `()`, `T: Serialize`, `Frame`, and `S: Stream<Item = Result<T, E>>` with `T:
Serialize`, `E: Into<CallError>`. An `Err` item runs `dispatch_late` and writes
`{"id","error":{..}}` as the stream's end, which `Tracked` reports `CutOff(None)` for, matching
§2.6's "an `error` envelope" end marker.

**R17. The standalone server is an HTTP/1.1 server, and the design should say which one.**
(Read.) "runs its own minimal HTTP/1.1 upgrade server" (§3.5) leaves a build agent to hand-roll
request parsing or pick hyper. hyper's `http1::Builder` with upgrades enabled is what
`ulo-http-hyper` already drives (Read, `transports/DIVERGENCES.md:653-661`), so `fw_ws::Server`
should reuse it: `ulo-net` listeners and `Tls`, the same `header_timeout` and `handshake_timeout`
settings with the same zero refusals, one `AppService`-free path that answers 404 off the gateway
paths and the 101 on them. This also makes `fw-ws` depend on `ulo-net` and hyper, which §1 can
list.

**Q4. Who writes the `Upgrades` metadata?** `prepare_app` reads `mounted.module_meta::<Upgrades>()`
(Read, `crates/ulo-http/src/server.rs`, `upgrade_paths`), so some module must register the gateway
hand-off. The gateway's mount cannot (R9). Options: a `fw_ws::WsModule` the application imports
once, which also binds `Rooms` and the broadcast adapter (R9, R10); or the `#[module]` macro
recognising a gateway among `controllers`, which it cannot without knowing the type. The first is
one import and is consistent with §2.10's rule being about inputs alone. Decide, and write it into
§4.1's example.

**Q5. What fires each `DisconnectReason`?** `ClientClose { code, reason }`, `ServerClose { code }`,
`ProtocolError`, `Drain` and `Lost` are listed without a mapping from what tungstenite reports: a
`Message::Close(Some(frame))` read, a close the gateway sent, `Error::Protocol`/`Error::Utf8`/
`Error::Capacity`, the drain's 1001, and `Error::ConnectionClosed`/`Error::AlreadyClosed`/an
`io::Error`. Confirm that mapping, and whether a close the server sent after a guard's refusal
reaches `on_disconnect` at all (the connection never passed `on_connect`).

**Q6. Server-side keep-alive.** tungstenite sends no Ping on its own. Does a gateway get
`ping_every: Bound` and `pong_timeout: Bound` (`Default` meaning off, as `max_inflight` means no
bound), closing with 1001 when a Pong is late? Without it a half-open connection is noticed only at
the next write, which §4.2's load-shedding and drain rows assume is sooner.

## §5 RPC

**R18. The `Link` SPI is three methods short of the core's lifecycle.** (Read.) `Link` has
`capabilities`, `listen` and `connect` (§5.3). `fw_rpc::Server<L>` implements the core `Server`,
whose `prepare` must refuse what it can see, whose `drain` receives the token and whose `close` is
bounded (Read, `crates/ulo/src/transport/server.rs:24-69`). TCP's endpoint text and `Tls` resolve in
`prepare` (§2.7); a broker URL that does not parse is the same class. Change: `Link` gains
`fn prepare(&mut self) -> impl Future<Output = Result<(), BoxError>> + Send` (defaulted `Ok`),
`fn drain(&self) -> impl Future<Output = ()> + Send` (TCP `goaway`, NATS drain, AMQP
`basic.cancel`, MQTT unsubscribe, Kafka pause, as §5.3 lists them), `fn close(&self) -> impl
Future<Output = Result<(), BoxError>> + Send`, and `fn bound(&self) -> Vec<BoundAddr>` (defaulted
empty) so `App<Bound>::addresses()` reports a TCP or UDP port 0. The `Inbound`, `Outbound`,
`Delivery`, `ReplyPath` and `Ack` types named in §5.3 need their shapes: `Inbound: Stream<Item =
Delivery> + Send`, `Delivery { frame: Frame, reply: Option<ReplyPath>, ack: Ack }` with `ReplyPath::
send(&self, Frame) -> BoxFuture<'static, Result<(), BoxError>>` and `Ack::{ack, reject}` (no-ops
off AMQP and Kafka), `Outbound { send(pattern, Frame, reply_to) , replies: BoxStream<Frame> }`, and
`Capabilities { binary: bool, max_frame: Option<u64>, ordering: Ordering, shapes: &'static [Shape],
native_backpressure: bool, delivery: Delivery }` (R22 for the last field).

**R19. The frame grammar needs its literals, its length prefix and its codec defaults.** (Read.)
§5.2 names the fields `t,id,p,h,d,e,n` and the frame kinds, not the `t` values: write them as the
kind names (`"req"`, `"evt"`, `"res"`, `"err"`, `"item"`, `"end"`, `"open"`, `"in"`, `"in_end"`,
`"cancel"`, `"credit"`, `"goaway"`). `id` is a `u64` the caller allocates per connection on TCP and
UDP (brokers carry the correlation natively, Q7). TCP framing is "length-prefixed": state the
prefix as a 4-byte big-endian length bounded by `max_frame`, so an oversized prefix closes the
connection before the body is read. "JSON or CBOR depending on the link's codec" names no default
per link: the old links were JSON on every transport (Read, `git show
master:crates/ulo-rpc-nats/src/nats_adapter.rs`, the payload format), and no CBOR crate is in the
local registry (Read, no `ciborium`, `minicbor` or `serde_cbor` under the registry). Default every
link to JSON, with `binary: false`, and make CBOR the opt-in `codec = cbor` once a crate is chosen
(Q8). `Bytes` on a JSON link is then refused in `prepare` for a handler and before I/O for a
client, as §5.2 already says.

**R20. Decide what a broker carries: the envelope or the payload.** (Read.) §5.2 says every link
speaks one grammar; §5.3's table maps request-reply onto each broker's own correlation
(`_INBOX`, `reply_to`, Response Topic, a reply topic plus a header). If the whole frame travels as
the broker message body, `p` duplicates the subject, `h` duplicates native headers, and `nats req
order.create '{..}'` from the CLI no longer reaches a handler, which the old link accepted (Read,
master `nats_adapter.rs`, lines 20-47). If the raw `d` travels, with `p` as the subject, `h` as
native headers and `id` as the native correlation, the request lane is interoperable and the reply
lane alone carries the envelope (`res`, `err`, `item`, `end`), a raw body without a reply address
is an `evt` and one with it is a `req`. The second is what the table already describes; §5.2 should
say it, since the conformance suite's wire assertions depend on it. Q7.

**R21. The no-destination signals are incomplete, and one is conditional.** (Spec; Read.) §5.2 lists
NATS, Redis, AMQP and Kafka. MQTT v5 has a signal §5.2 omits: PUBACK reason code 0x10 "No matching
subscribers" on a QoS 1 publish, PUBREC 0x10 on QoS 2, and none on QoS 0 (Spec, MQTT v5 §3.4.2.1,
§3.5.2.1; rumqttc exposes it, Read `rumqttc-0.25.1/src/v5/mqttbytes/v5/puback.rs:8`). Kafka's
`UNKNOWN_TOPIC_OR_PARTITION` arrives only where the broker's `auto.create.topics.enable` is off;
with it on, the default, a publish to an unhandled pattern creates the topic and the client waits
to its timeout (Spec, Kafka broker configuration; the old link pre-created topics for this reason,
Read master `kafka_adapter.rs`, "Create the handler topics up front"). So on Kafka with auto-create,
and on MQTT at QoS 0, a miss is a client `Timeout`, not `Unavailable`, and no mapping changes that.
Change: §5.2's uniformity claim becomes "`Unavailable` where the link signals no destination,
`Timeout` where it cannot", `Capabilities` declares `miss_signal: bool`, and the conformance suite
asserts `Unavailable` where it is `true` and `Timeout` where `false`. Redis's `PUBLISH` receiver
count holds as written (Read, `redis-1.3.0/src/commands/mod.rs:1608`). AMQP's `basic.return` on a
`mandatory` publish arrives on the channel asynchronously; lapin surfaces it through
`wait_for_confirms` under confirm mode (Read, `lapin-4.10.0/src/channel.rs:318-324`), so the
RabbitMQ link runs its client channel in confirm mode, which §5.3 should state.

**R22. Delivery to two instances differs per broker, and the design is silent.** (Spec; Read.) A
request published on NATS reaches every plain subscriber unless the servers share a queue group
(Spec, NATS queue groups; `queue_subscribe`, Read `async-nats-0.46.0/src/client.rs:675`); MQTT
delivers to every subscriber unless they use a shared subscription `$share/{group}/{filter}`,
whose availability the CONNACK announces (Spec, MQTT v5 §4.8.2); Redis Pub/Sub delivers to every
subscriber and has no competing mode (Spec); AMQP queues and Kafka consumer groups compete by
construction. Two server instances on NATS, MQTT or Redis would each answer one request. Change:
`Capabilities::delivery: Competing | FanOut | Addressed` (TCP and UDP are `Addressed`); the NATS
link subscribes through a queue group and the MQTT link through `$share/<group>/`, each with a
`group` setting defaulting to one name (`"ulo"`), both making them `Competing`; Redis stays
`FanOut`, documented, and `RpcClient` drops a second reply for an `id` it already answered. The
conformance suite's two-instance case asserts the bound the capability declares, as the old
suite did (Read, master `ulo-rpc-conformance/src/lib.rs:553-559`).

**R23. The binary flag is not on `Shape`, so the refusal reads another record.** (Read.) `Shape`
is `Unary | ServerStreaming | ClientStreaming | Bidi` with no payload kind (Read,
`crates/ulo/src/transport/handler.rs`, `Shape`; `transports/DIVERGENCES.md:729-731`, "no binary
flag, which is RPC's and 2b's"). §5.2 has `prepare` refuse a binary handler "from the shape recorded
on its `HandlerSpec`". Change: the RPC attribute records `payload: PayloadKind::{Serde, Binary}` on
its handler value through an autoref probe over each `Payload<T>` parameter, `Payload<Bytes>` and
`Bytes` answering `Binary`, as the HTTP attribute's `HostProbe` records `Host<T>` (Read,
`crates/ulo-http/src/__private.rs`, `HostProbe`). `prepare` reads it from
`MountedHandler::handler::<RpcHandler>()`.

**R24. Flow control: defer `credit`, state the AMQP prefetch default and its flag.** (Spec.) `credit`
frames on NATS and Redis require both the client and the server to implement a window, and nothing
in §5.4's client mentions it. Reserve the frame kind and defer the mechanism; `max_inflight` sheds
on every link in the first version. On AMQP, `basic.qos` with `global = false` is per consumer and
with `global = true` per channel under RabbitMQ's reinterpretation of the field (Spec, RabbitMQ
"Consumer Prefetch"); the link sets a per-consumer prefetch whose default §5.3 should name (the
old link set none). A frame kind that no link sends in a release is still part of the grammar a
later link must honour, which is why reserving it is enough.

**R25. The conformance suite's `Broker` trait and scenario list.** (Read.) §5.2 lists eight
scenario names. The old suite's shape fits the new design and is the evidence of what a broker
test needs: `start()` once per case, the two halves under test, `disrupt()`, a `Budget` for
subscribe latency, one-way effects and recovery, and one macro stamping every case per link (Read,
master `ulo-rpc-conformance/src/lib.rs:54-110, 603-625`). The list a build needs: unary round trip;
a domain error as the `err` envelope with `kind`; a guard's refusal as `forbidden`; a panic as
`internal`; a `Payload<T>` that fails to decode as `bad_request`; headers reaching `CallHeaders`;
an event reaching its handler with no reply; an unhandled pattern as `Unavailable` or `Timeout`
per R21; an unhandled event logged and acknowledged; a server stream in order ending with `end`;
a client stream; a bidi stream; `cancel` mid-stream firing `ClientCancelled` and stopping the
producer; `deadline-ms` firing `Deadline` and rendering `timeout`; the client's own timeout as
`Timeout`; a binary payload round trip where `binary` and the pre-I/O refusal where not; an
oversized payload as `payload_too_large`; the drain: a new call refused `unavailable`, an in-flight
call finishing, a stream ending on `draining()`; recovery after `disrupt()` on the brokers; two
instances and the delivery bound of R22. A case holds a `TypeName`-free assertion: the kind and
the reason strings, never the message text.

**R26. Settings, in the vocabulary, with TLS per link.** (Read.) `max_inflight: Count` on
`fw_rpc::Server`, `max_frame: u64` per link, the client module's default timeout as `Bound`
(`RpcClientModule::for_root(link).timeout(Bound::After(..))`, `Default` five seconds as before),
the per-call `.timeout(Duration)` staying a `Duration` since it is a value `prepare` never sees,
`Count::Max(0)` and `Bound::After(ZERO)` refused in `prepare` with the HTTP hints. TLS: TCP takes
`ulo_net::Tls` with ALPN none; the brokers take their URL schemes, `tls://` for NATS, `rediss://`
for redis, `amqps://` for lapin, `mqtts://` for rumqttc, and `security.protocol=SSL` for rdkafka,
each a client-library feature the link crate enables; the registry holds `rustls-connector` for
lapin and `rustls-native-certs` and `webpki-roots` for the others (Read, registry listing).
"Broker links take their client libraries' TLS configuration" (§2.7) is right; the schemes are
what a build agent has to know.

**R27. An unhandled event's acknowledgment per broker.** (Spec.) §5.2 says "acknowledged as
rejected": on AMQP that is `basic.reject` with `requeue = false` (Spec, AMQP 0-9-1, the `basic`
class; `basic.nack` is RabbitMQ's extension), on Kafka the offset is committed, on MQTT nothing further than the QoS
acknowledgment exists, and on NATS and Redis nothing is acknowledged. Say the four so the links
agree.

**Q7. Envelope or payload on the brokers?** R20 recommends the payload with native correlation;
the alternative is the whole frame as the body on every link. The answer fixes the conformance
suite's wire assertions and the interoperability story.

**Q8. Which CBOR crate, when one is added?** `ciborium`, `minicbor` and `serde_cbor` are all
absent from the local registry; the first two are maintained. R19 defaults every link to JSON so
the choice does not block the build.

**Q9. Is a client `Timeout` acceptable as the miss on Kafka-with-auto-create and MQTT QoS 0?** R21
says yes and has `Capabilities` declare it. The alternative is pre-creating topics as the old link
did, which keeps `UNKNOWN_TOPIC_OR_PARTITION` only where a handler never existed and still answers
`Timeout` for one whose topic exists on a stopped server.

**Q10. The competing-consumer group name.** One default (`"ulo"`) on NATS and MQTT, or the
application's root module name, or required?

## §6 gRPC

**R28. Choose the server engine, and name what each choice costs.** (Read.) §6.2 writes "GOAWAY
through hyper's graceful shutdown" and §2.7 gives gRPC `fw_net::Tls` with ALPN `h2`. tonic's own
`transport::Server` owns the socket and the accept loop, has its own TLS (`tls_config`, behind a
feature) and its own `max_concurrent_streams` and `concurrency_limit_per_connection`, and parses
`grpc-timeout` in a middleware (Read, `tonic-0.14.6/src/transport/service/grpc_timeout.rs:14, 103`);
the old adapter used it (Read, `git show master:crates/ulo-grpc/src/grpc_adapter.rs`). Using it means
`ulo-net`'s endpoints, inherited sockets and `Tls` are bypassed for gRPC and TLS is configured
twice in two vocabularies. The alternative is the HTTP backend's own accept loop over `ulo-net`
listeners with hyper's `http2::Builder` and `tonic::service::Routes` (or R29's dispatcher) as the
service, which keeps one TLS story and one activation story but duplicates `ulo-http-hyper`'s
accept loop (Read, `transports/DIVERGENCES.md:653-667`) unless that loop is factored into a shared
crate. Under that choice `grpc-timeout` is parsed by `fw-grpc` itself (Spec, gRPC over HTTP/2:
`TimeoutValue` of one to eight digits and `TimeoutUnit` in `H M S m u n`). Recommendation: the
second, with the accept loop factored out of `ulo-http-hyper` into `ulo-hyper-serve` (listeners,
TLS, handshake and header timeouts, the per-connection `JoinSet`, the graceful drain), consumed by
`ulo-http-hyper`, `fw-grpc` and `fw-ws`'s standalone server (R17). Q11 if the author prefers tonic's
transport.

**R29. Say how a `Method` marker becomes a service without tonic's generated trait.** (Read.) §6.1
binds handlers to markers and never implements the `UserService` trait tonic generates. The
mechanism exists in tonic's public API: `tonic::server::Grpc::new(codec)` with `unary`,
`server_streaming`, `client_streaming` and `streaming`, each taking a service type implementing the
matching `UnaryService`/`ServerStreamingService`/.. trait, which is what tonic's generated code
calls per method (Read, `tonic-0.14.6/src/server/grpc.rs`). `fw-grpc` builds one tower service
dispatching on `:path` to the marker's `Grpc` call with `ProstCodec<M::Request, M::Response>`, and
answers `UNIMPLEMENTED` off the table, as §6.1 requires. `Grpc::Reply` is then
`http::Response<tonic::body::Body>`: an interceptor reads and writes reply metadata through its
headers and cannot read the message, which is the shape the old `GrpcReply` settled on (Read,
`CLAUDE.md`, the `GrpcReply` paragraph). A stream item's `Err` goes through `dispatch_late` and is
written as the trailers `grpc-status`/`grpc-message`/`grpc-status-details-bin`, which is gRPC's
mid-stream error form (Spec, gRPC over HTTP/2: Trailers-Only is not applicable once data frames
were sent; status travels in the trailers). Both sentences belong in §6.2.

**R30. The build step: protox is not available offline, and markers want a `ServiceGenerator`.**
(Read.) `protox` is absent from the local registry; `protoc-bin-vendored 3.2.0` with its per-platform
binaries is present, and the old `ulo-build` used it behind a default-on `vendored-protoc` feature
yielding to `PROTOC` (Read, registry listing; `git show master:crates/ulo-build/Cargo.toml`).
[34] asks for "no system protoc", which a vendored binary meets. Until protox is fetched,
`fw-grpc-build` can only be built with the vendored protoc (Q11). For the markers, tonic-prost-build
exposes its `ServiceGenerator` (Read, `tonic-prost-build-0.14.6/src/lib.rs:948`) and prost-build
takes one through `Config::service_generator`; a generator wrapping tonic's and appending
`pub struct GetUser; impl fw_grpc::Method for GetUser { .. }` per method under the service's
module writes the markers in the generated file, where `fw_grpc::include_proto!` finds them, and
replaces the old crate's post-hoc file rewrite (Read, master `ulo-build/src/lib.rs`, `SENTINEL`).
The descriptor set for reflection comes from `file_descriptor_set_path` (Read, `lib.rs:634`).

**R31. Write the HTTP-status-to-gRPC table whole.** (Spec.) §6.2's "(401 to UNAUTHENTICATED, 403
to PERMISSION_DENIED, 429 and 502–504 to UNAVAILABLE, and so on)" leaves the rest to the build. The
specification's table (gRPC `http-grpc-status-mapping.md`): 400 → INTERNAL, 401 → UNAUTHENTICATED,
403 → PERMISSION_DENIED, 404 → UNIMPLEMENTED, 429 → UNAVAILABLE, 502 → UNAVAILABLE, 503 →
UNAVAILABLE, 504 → UNAVAILABLE, every other status → UNKNOWN. The 404 → UNIMPLEMENTED row matters
because a pre-dispatch layer answering 404 would otherwise be guessed as NOT_FOUND.

**R32. Details packing, named by library.** (Read.) `grpc-status-details-bin` carries a
`google.rpc.Status` whose `details` repeat the code and message (Spec, gRPC richer error model).
tonic-types' `ErrorDetails` maps the design's `Detail` variants one to one: `FieldViolations` →
`set_bad_request`, `ErrorInfo` → `set_error_info`, `RetryAfter` → `set_retry_info`, `Help` →
`set_help` (Read, `tonic-types-0.14.6/src/richer_error/error_details/mod.rs:447-805`), and
`StatusExt::with_error_details` writes the trailer. `Detail::Json` has no tonic-types setter: pack
a `prost_types::Value` built from the `serde_json::Value` into a `prost_types::Any` with type URL
`type.googleapis.com/google.protobuf.Value` and append it to the `Status.details` list by hand.
`Message<T>`, `Request<T>` and `Streaming<T>` all declare `CONSUMES_BODY = true` so two takers fail
the pairwise check naming both, as the old crate did (Read, `CLAUDE.md`, gRPC params).

**R33. gRPC settings in the vocabulary, and mTLS is blocked by F307.** (Read.) `max_inflight:
Count`, `max_per_connection: Count`, `max_concurrent_streams: Count`, each refusing `Max(0)` in
`prepare` with the HTTP hints; `tls: ulo_net::Tls` with ALPN `h2` only. §6.1's example guard is
`MtlsGuard`, and `ulo-net`'s `Tls` has no client-certificate verification (Read,
`transports/DIVERGENCES.md:648-651`, F307). Either F307 is built in 2b (a `Tls::client_auth(roots)`
whose verifier `ConnInfo::tls` reports the peer certificate through) or the example changes to a
token guard. Q13.

**R34. Health and reflection, as built against the libraries.** (Read.) `tonic_health::server::
health_reporter()` answers a `HealthReporter` with `set_service_status(name, ServingStatus)` (Read,
`tonic-health-0.14.6/src/server.rs:21, 74`); `fw-grpc` binds it as `Dep<GrpcHealth>` wrapping the
reporter, sets every known service SERVING after `bind` and NOT_SERVING in `drain`, as §6.2 says.
Reflection: `tonic_reflection::server::Builder::configure().register_encoded_file_descriptor_set(..)
.build_v1()` and `.build_v1alpha()` (Read, `tonic-reflection-0.14.6/src/server/mod.rs:50-100`),
both added to the route table in debug builds and behind `reflection(true)` in release, per
decision 7. The descriptor bytes come from `include_bytes!(concat!(env!("OUT_DIR"), "/<name>.bin"))`,
which `fw_grpc::include_proto!` can emit beside the module when `fw-grpc-build` wrote the set.

**Q11. protox, or the vendored protoc?** R30 states the offline fact. If protox is the answer,
the build waits on a fetch and `fw-grpc-build` is the one crate the first compile cannot reach.

**Q12. Is gRPC on the HTTP server's port, or on an embedding host, in scope?** The brief asks what
the embedding pattern means for gRPC mounted on HTTP. §6 gives gRPC its own `Server` and port, and
nothing in §3.8 routes `application/grpc` requests to it. Serving gRPC on the HTTP port would need a
hand-off like `UpgradeHandler` keyed by path and content type rather than by `Upgrade`, and an
embedded host would need the same. Recommendation: out of scope for 2b, written as a limit in §6,
with the hand-off shape noted as the extension point if it is ever wanted.

**Q13. mTLS now or later?** R33.

**Q14. Does `fw-grpc` reuse `ulo-http`'s pre-dispatch stage?** §6.2 gives gRPC `.apply`,
`.apply_for`, `.layer` and `.layer_for` "as HTTP's does". `ulo_http::PreDispatch`, `Middleware`,
`Next`, the scope patterns and the tower bridge are HTTP's types over `ulo_http::Request` (Read,
`crates/ulo-http/src/pre_dispatch.rs`, `middleware.rs`, `tower_bridge.rs`). A gRPC request is an
`http::Request` and fits `ulo_http::Request` with `upgrade: None`. Options: `fw-grpc` depends on
`ulo-http` and `PreDispatch` becomes `PreDispatch<T: Transport = Http>` so gRPC's entries are a
distinct metadata key (a 2a type gains a defaulted parameter); or `fw-grpc` duplicates the stage.
The first is one change in `ulo-http` and no duplication. §3.4's sentence "The WebSocket and RPC
stages take `Middleware<T>` only" names a generic `Middleware<T>` that does not exist and a stage
§4 and §5 never describe; strike it or define it (R46).

## §7 GraphQL

**R35. The context type is a parameter of the engine adapter, not a convention.** (Read.) §7 says
"Each engine adapter builds the schema context per execution through `exec.get::<GqlContext>()`",
naming the user's type as if the adapter knew it. async-graphql takes context as `Request::data(D)`
for any `D: Any + Send + Sync` (Read, `async-graphql-7.2.1/src/request.rs:90`); juniper's `Context`
is a type the schema's root types name (Read, `juniper-0.16.2/src/executor/mod.rs:404`). Change:
`AsyncGraphql<S, C>` with `C: Send + Sync + 'static`, bound as `AsyncGraphql<ApiSchema, GqlContext>
as dyn Engine`, resolving `exec.get::<C>()` per call and inserting it with `Request::data`, plus
the `ExecutionRef` itself as a second `data` so `fw_graphql::dep::<T>(ctx)` reads
`ctx.data::<ExecutionRef>()?.get::<T>()`; `Juniper<Q, M, Sub>` resolving `exec.get::<Q::Context>()`
and handing it as the context, where `dep` is the user's `Context` struct holding an `ExecutionRef`
field. `fw_graphql::dep` is therefore engine-specific and belongs in each adapter crate under one
name, not in `fw-graphql`.

**R36. The GraphQL-over-HTTP status rules, written out.** (Spec.) The specification decides: a
response in `application/graphql-response+json` answers 200 for a request that executed, errors
included, and 4xx for a request that failed before execution (a document that does not parse or
validate, a missing `query`, an unparseable JSON body), 400 being the status for those; in the
legacy `application/json` media type every response is 200. A GET request names `query`,
`operationName`, `variables` (JSON text) and `extensions` (JSON text) as query parameters; a GET
whose operation is a mutation is refused with 405. The server negotiates the response media type
from `Accept`, answering `application/graphql-response+json` when accepted and `application/json`
otherwise, and refuses a POST whose body is not `application/json` with 415, as the specification
recommends. §7 names the spec; the
build needs the rows, and the HTTP controller's `IntoReply<Http>` for `GqlResponse` applies them,
which needs `GqlResponse` to carry whether it is a request error (R39).

**R37. The graphql-transport-ws gateway is a hand-written `Gateway`, and the design should list what
it owns.** (Spec; Read.) The protocol's messages have no `id` on `connection_init`, `ping` and
`pong` while they expect an answer, and `complete` from the client cancels an operation (Spec,
PROTOCOL.md), so the `#[fw_ws::message]` convention of §4.2 ("a message without an `id` is
fire-and-forget") does not fit; the old implementation was a manual gateway for the same reason
(Read, `git show master:crates/ulo-graphql-async-graphql/src/subscription_gateway.rs`). The gateway
owns: the subprotocol echo (R13); `connection_init` once, a second one closing 4429, the payload
kept in `Session` for the context builder; `connection_ack`; a `subscribe` before the ack closing
4401; a duplicate `id` closing 4409; an unparseable message closing 4400; an init that does not
arrive within `connection_init_timeout: Bound` closing 4408, timed by the app's `Timer`; `ping`
answered with `pong` and `pong` accepted in both directions; one execution per `subscribe` whose
stream writes `next` items and `complete`, or `error` with an array of GraphQL errors; `complete`
from the client cancelling that execution with `ClientCancelled`; the context built per execution
through R35. async-graphql ships a protocol driver for exactly this (Read,
`async-graphql-7.2.1/src/http/websocket.rs:109`), which the async-graphql adapter may wrap, but the
juniper path has none in the registry, so the gateway is written once in `fw-graphql-ws` over
`Engine::subscribe`.

**R38. The playground.** (Read.) async-graphql's `GraphiQLSource::build().endpoint(..).subscription_
endpoint(..).finish()` and juniper's `graphiql_source(endpoint, subscriptions)` both answer the HTML
(Read, `async-graphql-7.2.1/src/http/graphiql_v2_source.rs:62`, `juniper-0.16.2/src/http/
graphiql.rs:11`). `fw-graphql-http` serves one page from `Engine::playground_html(endpoint,
subscriptions) -> Option<String>` on a GET with `Accept: text/html` and no `query` parameter, under
`GraphqlConfig::playground(bool)` defaulting to `cfg!(debug_assertions)`; the method joins the
`Engine` SPI, since the page is the engine's.

**R39. `GqlResponse` needs a request-error flag, and `Engine::execute` needs the HTTP head.** (Read;
Spec.) R36's status rule reads whether execution began, which a serialized `{data, errors,
extensions}` does not say. `GqlResponse { outcome: Executed | RequestError, data, errors,
extensions }`, serialized without `outcome`. `Engine::execute(&self, req: GqlRequest, exec:
ExecutionRef)` has no access to the request's headers for a resolver that reads one; the HTTP
controller seeds `Dep<RequestHead>` into the execution already (Read,
`crates/ulo-http/src/service.rs`, `respond`), so a context built from `exec.get::<C>()` reads it
through `Dep<RequestHead>`, and the SPI needs no change. Say so in §7, since it is the answer to
"how does the context see the request".

**Q15. Juniper subscriptions over `resolve_into_stream`.** juniper 0.16 answers a subscription as a
`Connection` of values through `resolve_into_stream` (Read, `juniper-0.16.2/src/lib.rs:244`); the
`juniper_subscriptions` crate is not in the registry. Is the juniper adapter's `Engine::subscribe`
built on `resolve_into_stream` directly, or is juniper subscription support deferred?

**Q16. Two engines in one application.** `AsyncGraphql<S, C> as dyn Engine` binds one engine under
one key; a second schema needs a qualifier (`as dyn Engine @ Admin`) and a second `GraphqlModule`
at another path reading `Dep<dyn Engine, Admin>`. Is that the spelling, or is one engine per
application the stated limit?

## §8 Runtime

Nothing to refine: the crate is built to §8 (Read, `crates/ulo-tokio/src/lib.rs`). One
observation for §9: `shutdown_signal` installs its handlers at first poll inside the runtime (Read,
`lib.rs`, the doc comment), which is what a restarted child relies on when `ulo dev` sends SIGTERM.

## §9 Development command

**R40. The crate and the command have names already.** (Read.) §1 lists `fw-dev (installs cargo
fw)`; the repository has `ulo-cli` with `ulo new`, `ulo generate` and `ulo dev` behind a default-on
`dev` feature over watchexec (Read, `git show master:crates/ulo-cli/Cargo.toml`; watchexec 8.2.0
and `ignore-files` are in the registry). Either `ulo-cli` keeps the command and §1 names it, or
`cargo ulo dev` is a second entry point. Recommendation: keep `ulo dev` in `ulo-cli` and amend §1;
a cargo subcommand is an alias (`cargo-ulo`) if wanted later.

**R41. The `LISTEN_PID` trampoline, concretely.** (Read.) The old command set `LISTEN_FDS` and
removed `LISTEN_PID` on purpose, because `pre_exec` runs between fork and exec where only
async-signal-safe calls are allowed and `setenv` allocates (Read, `git show
master:crates/ulo-cli/src/commands/dev.rs`, lines 36-70). `ulo-net` now requires `LISTEN_PID` to
equal the child's pid (T11). The trampoline that meets both: the command re-executes its own binary
as `ulo __exec -- <child> <args>`, which runs after exec with a normal allocator, sets
`LISTEN_PID` to `std::process::id()`, and `exec`s the child; `exec` keeps the pid, so the variable
equals the child's. No `sh` is involved and nothing runs between fork and exec but `dup2` and
`fcntl` for fd 3, as before. §9 should name this rather than "a small exec trampoline".

**R42. Clearing `FD_CLOEXEC` and setting it again.** (Read.) The held socket must survive the
command's exec of the trampoline and the trampoline's exec of the child, so `FD_CLOEXEC` is clear
on fd 3 through both; `ulo-net` sets it in the child (§2.7). The old code's `place_listen_fd` with
its `dup2`-equal-descriptor case is the working form (Read, master `dev.rs`, lines 36-62 and the
tests at 398-485) and can be carried over as written.

**Q17. How does `FW_DEV=1` turn an `Addr` endpoint into an inherited one?** §9 says the command
"enables [inherited endpoints] automatically through `FW_DEV=1`", and nothing says how an app
written `Server::new("0.0.0.0:8080")` learns to adopt fd 3. Two mechanisms are possible: `ulo dev
--listen 0.0.0.0:8080` binds that address and passes `LISTEN_FDNAMES=0.0.0.0:8080`, and under
`ULO_DEV=1` `prepare` resolves an `Endpoint::Addr` whose text equals an inherited name to that
socket; or the application writes `Endpoint::inherited("http")` itself and the command takes
`--listen http=0.0.0.0:8080`. The first keeps `main` unchanged between development and production,
which is the feature's point; it puts an environment read into `EndpointSpec::resolve`, which
`prepare` already runs (Read, `crates/ulo-net/src/endpoint.rs`, `EndpointSpec::resolve`). Decide,
and name the variable `ULO_DEV`.

**Q18. UDP sockets under `ulo dev` and F306.** The activation vets every inherited descriptor as a
listening TCP socket and refuses the lot on one that is not (Read,
`transports/DIVERGENCES.md:639-644`, F306). An RPC application on UDP cannot hold its socket across
restarts until F306 is settled. Is UDP excluded from `--listen` in 2b, or does F306 get built with
the UDP link?

## Cross-cutting

**R43. Move the `prepare` failure helpers out of `ulo-http` so every transport builds its
`PrepareError` one way.** (Read.) `Failures`, `Failure::naming`, `Names`, `check_zero_timeouts` and
`check_zero_counts` are crate-private in `crates/ulo-http/src/server.rs`. WebSocket, RPC, gRPC and
GraphQL each refuse duplicates naming controllers, zero counts and zero bounds; copying the helpers
four times is four spellings of the hint. Change: `ulo_transport::prepare::{Failures, Failure,
Names, zero_bound(setting, bound, effect), zero_count(setting, count, effect)}`, public, with
`ulo-http` switched to them. The refusal texts then read alike on every transport, which §12
promises.

**R44. One settings vocabulary for 2b, stated once.** (Read.) HTTP settled it by example:
`body_limit: u64`, `max_inflight: Count`, `max_concurrent_streams: Count`, `header_timeout: Bound`,
`timeout_grace: Bound`, `shed_retry_after: Duration` (Read, `crates/ulo-http/src/server.rs`, the
builder). The rule: a byte size is `*_limit: u64`; a count is `max_*: Count`; a time that may be
off is `*_timeout: Bound` or a `Bound` named for what it bounds; a time that is a value, never
off, is a `Duration`. §4–§6 then read `message_limit`, `max_connections`, `max_inflight`,
`max_outbound`, `ping_every`, `max_frame`, `max_per_connection`, and a `Timeout`-like metadata type
per transport where one exists. A sentence in §2.8 or §13 saves a sentence per setting later.

**R45. Name every transport's `Reply` and its miss source.** (Read.) HTTP has `Response` and the two
public miss sources `NoRoute` and `MethodNotAllowed` so an error handler tells a miss from a
handler's own kind (Read, `crates/ulo-http/src/miss.rs`). The others need the same: `fw_ws::Reply`
(R16) and `fw_ws::NoHandler` as the source of the `Unimplemented` an unknown event raises through
`recover(None, ..)`; `fw_rpc::Reply` and `fw_rpc::NoHandler` as the source of the `Unavailable` an
unhandled pattern raises on TCP and UDP; `fw_grpc::Reply = http::Response<tonic::body::Body>` (R29),
with UNIMPLEMENTED written by the dispatcher and offered to no handler, as HTTP's `OPTIONS` is.

**R46. Strike or define the WebSocket and RPC pre-dispatch stages.** (Read.) §3.4's last sentence
gives WebSocket and RPC a stage taking `Middleware<T>`; §4 and §5 describe none, `Middleware` is not
generic (Read, `crates/ulo-http/src/middleware.rs:39`), and the decision in §13.1 is one stage,
HTTP's. Recommendation: strike the sentence; guards and interceptors cover WebSocket and RPC, and
gRPC's stage is Q14's.

**R47. Core and `ulo-http` extensions for 2b, numbered.** (Read.) So the build logs cite them as 2a
cited X1–X18: X19 `Inputs::also_seeded_by` with the freeze merging equal seeder sets (R7); X20
`AppHandle::mounted::<T>()` and `Mounted::handlers_of::<U>()` (R5, R6); X21 the four lifecycle
methods on `ulo_http::UpgradeHandler`, called by `Server<B>` and `Embedded<A>` (R5); X22 the
`prepare` helpers in `ulo-transport` (R43); X23 `PreDispatch<T = Http>` if Q14 takes reuse. X19 and
X20 are additive core changes; X21–X23 touch `ulo-http` and `ulo-transport` and no core signature.

**R48. Spans for WebSocket and the brokers.** (Spec.) §2.9 gives HTTP, RPC and gRPC their names.
WebSocket has no OpenTelemetry convention; name the span after the event, `ws.event = "chat.send"`,
`otel.name = "chat.send"`, with `ulo.transport = "ws"`, and the connect phase `ws.connect` with
`url.path`. `messaging.system` values follow the semantic conventions' registry: `nats`, `redis`,
`rabbitmq`, `mqtt`, `kafka`; TCP and UDP record none. `span::call` already declares every field
empty (Read, `crates/ulo-transport/src/span.rs`), so no new field is needed beyond `ws.event`,
which joins the list.

**R49. The dependencies the offline registry lacks, in one place.** (Read.) `protox` (R30), a CBOR
crate (R19), a MessagePack crate (R11), `juniper_subscriptions` (Q15). Everything else §4–§9 names
is present at the versions listed in the header. A build that cannot fetch picks R30's vendored
protoc, R19's JSON default and R11's text-only default, and the three codecs become follow-ups.

**Q19. Which codec crates, once fetched?** R19 and R11 default to JSON and text so the build is not
blocked.

**Q20. HTTP-only metadata on a non-HTTP handler.** `#[meta(Timeout(..))]` and `#[meta(BodyLimit(..))]`
are inert values (Read, `crates/ulo/src/transport/metadata.rs`), so one written on an RPC handler
compiles and does nothing. HTTP's `prepare` sees only HTTP handlers through `Mounted`, and
`AppHandle::handlers()` lists every handler's `entries()` with its transport key (Read,
`crates/ulo/src/app/handle.rs:109`, `handler.rs`, `HandlerInfo::entries`). Should `ulo_http::
Server::prepare` refuse an HTTP metadata type found on a handler of another transport, or is an
inert declaration accepted?

## Not verified here

- The host-by-host moment a dropped response body is observed on poem, actix and rocket under the
  new adapters (R1, Unverified); the suite's cancellation scenario decides it.
- That tungstenite answers a Ping with a Pong carrying the same payload without the caller
  flushing explicitly; the old adapter polled the read side once more after a Close to put the
  reply on the wire (Read, master `ulo-ws-tungstenite/src/adapter.rs`, the comment on
  `Message::Close`), which suggests the write of a queued control frame needs a poll the gateway
  loop must give it.
- tonic's `server::Grpc` and the four shape-service traits being sufficient to dispatch without
  the generated trait (R29) is Read from tonic's generated code pattern and not compiled here.
- rdkafka's `UNKNOWN_TOPIC_OR_PARTITION` surfacing on a produce with auto-create off (R21), and the
  broker default of `auto.create.topics.enable` (Spec, Kafka documentation).
- Rocket's `Shutdown` handle being obtainable before `launch()` through `Rocket<Ignite>` (R3).

---

## Race 2b: build areas, files and contracts

In the form of `BUILD_PLAN.md` and `BUILD_PLAN_2A.md`. The rules of `BUILD_PLAN_2A.md` apply
unchanged: no cargo, no tests, own files only, frozen contracts, every divergence logged in
`divergences/race2b-<area>.md`, the coordinator commits. The spine step comes first and alone, as
in 2a; the eight areas then fill in parallel.

### S. The 2b spine (first, alone)

| File | Holds |
| --- | --- |
| `crates/ulo/src/transport/inputs.rs`, `crates/ulo/src/graph/wire.rs` | X19: `Inputs::also_seeded_by`, the merge of equal seeder sets in `declare_transport_inputs`, `InputDecl.seeders` |
| `crates/ulo/src/transport/server.rs`, `crates/ulo/src/app/handle.rs`, `crates/ulo/src/lib.rs` | X20: `AppHandle::mounted::<T>()`, `Mounted::handlers_of::<U>()` |
| `crates/ulo-transport/src/prepare.rs`, `crates/ulo-transport/src/lib.rs` | X22: `Failures`, `Failure`, `Names`, `zero_bound`, `zero_count`, moved from `ulo-http` |
| `crates/ulo-http/src/upgrade.rs`, `server.rs`, `embed.rs` | X21: `UpgradeHandler::{prepare, bound, drain, close}` and their call sites; `server.rs` switched to X22 |
| `crates/ulo-http/src/pre_dispatch.rs`, `middleware.rs` | X23 if Q14 takes reuse: `PreDispatch<T: Transport = Http>` |
| `Cargo.toml` | the new members: every crate below |
| each new crate's `Cargo.toml`, `src/lib.rs` | the module tree and every public signature with `todo!()` bodies, as the 2a spine did |

Design sections: this review's R5, R6, R7, R43, R47, Q14.

### W. WebSocket

| File | Holds |
| --- | --- |
| `crates/ulo-ws/src/lib.rs`, `transport.rs` | `Ws`, `WsConnect`, `WsCx`, `ConnectCx`, `Reply`, the inputs `ConnectionInfo`, `UpgradeHead`, `SessionHandle` (X19) |
| `crates/ulo-ws/src/gateway.rs` | `GatewayConfig`, `GatewayRef`, `OnConnect`, `OnDisconnect`, `AfterInit`, `DisconnectReason`, `ConnectRefused` |
| `crates/ulo-ws/src/session.rs` | `Session<T>`, the session factory |
| `crates/ulo-ws/src/envelope.rs`, `codec.rs` | the JSON envelope, `Payload<T>`, `Frame`, reserved names |
| `crates/ulo-ws/src/connection.rs` | the read loop, the outbound queue, limits, close codes, the per-message execution, `open_terminal` on disconnect |
| `crates/ulo-ws/src/handoff.rs` | the `ulo_http::UpgradeHandler` impl, with X21's four methods |
| `crates/ulo-ws/src/server.rs` | the standalone `Server` over hyper http1 and `ulo-net` (R17) |
| `crates/ulo-ws/src/rooms.rs`, `broadcast.rs` | `Rooms`, `RoomsIn`, `BroadcastAdapter`, the in-memory adapter |
| `crates/ulo-ws/src/module.rs` | `WsModule` (Q4): `Upgrades` registration, `Rooms`, the adapter |
| `crates/ulo-ws-macros/src/lib.rs`, `gateway.rs`, `message.rs` | `#[gateway]`, `#[message]` over `ulo-handler-codegen` |
| `crates/ulo-ws/src/__private.rs` | `WsHandler`, the `Payload` probe |
| `crates/ulo-ws-redis/src/lib.rs` | the Redis `BroadcastAdapter` value |

Design sections: §3.5, §4, §10 (WebSocket column), §12 rows; this review's R5–R17, Q4–Q6.

### R. RPC core, the socket links

| File | Holds |
| --- | --- |
| `crates/ulo-rpc/src/lib.rs`, `transport.rs` | `Rpc`, `RpcCx`, `Reply`, `CallHeaders`, `LinkInfo`, `NoHandler` |
| `crates/ulo-rpc/src/frame.rs`, `codec.rs` | the grammar (R19), JSON codec, the CBOR seam |
| `crates/ulo-rpc/src/link.rs` | `Link`, `Inbound`, `Outbound`, `Delivery`, `ReplyPath`, `Ack`, `Capabilities` (R18, R22) |
| `crates/ulo-rpc/src/server.rs` | `Server<L>`: `prepare` (patterns, shapes, binary, zero counts), `bind`, `serve`, `drain`, `close` |
| `crates/ulo-rpc/src/dispatch.rs` | per-delivery execution, deadline, cancel, the four shapes, `dispatch_late` |
| `crates/ulo-rpc/src/client.rs`, `client_module.rs` | `RpcClient`, `RpcClientModule`, `RpcError`, duplicate-reply drop |
| `crates/ulo-rpc/src/extract.rs` | `Payload<T>`, `Inbound<T>`, `CallHeaders` as `FromCall<Rpc>` |
| `crates/ulo-rpc/src/__private.rs` | `RpcHandler`, the payload probe (R23) |
| `crates/ulo-rpc-macros/src/lib.rs`, `message.rs`, `event.rs` | `#[message]`, `#[event]` |
| `crates/ulo-rpc-tcp/src/lib.rs`, `link.rs` | the TCP `Link`: length prefix, `goaway`, `Tls` |
| `crates/ulo-rpc-udp/src/lib.rs`, `link.rs` | the UDP `Link`: one datagram per frame, unary and events |

Design sections: §5.1–§5.4, §10 (RPC column), §12 rows; this review's R18–R27, Q7–Q10.

### B. The broker links and the RPC conformance suite

| File | Holds |
| --- | --- |
| `crates/ulo-rpc-nats/src/lib.rs`, `link.rs` | queue-group subscribe, `_INBOX` replies, no-responders, drain |
| `crates/ulo-rpc-redis/src/lib.rs`, `link.rs` | pub/sub per pattern, receiver count, fan-out declared |
| `crates/ulo-rpc-rabbitmq/src/lib.rs`, `link.rs` | queues, `reply_to`/`correlation_id`, confirm mode and `mandatory`, prefetch, `basic.reject`, `basic.cancel` |
| `crates/ulo-rpc-mqtt/src/lib.rs`, `link.rs` | `$share` subscribe, Response Topic and Correlation Data, PUBACK 0x10, CONNACK limits |
| `crates/ulo-rpc-kafka/src/lib.rs`, `link.rs` | consumer group, reply topic, headers, pause and commit, topic creation setting |
| `crates/ulo-rpc-conformance/src/lib.rs`, `cases/*.rs` | `Broker`, `Budget`, the case list of R25, the stamping macro |

Depends on R's `Link` contract. Design sections: §5.2, §5.3; this review's R20–R22, R25–R27.

### G. gRPC

| File | Holds |
| --- | --- |
| `crates/ulo-grpc/src/lib.rs`, `transport.rs` | `Grpc`, `GrpcCx`, `Reply`, `GrpcMetadata`, `PeerAddr`, `include_proto!` |
| `crates/ulo-grpc/src/method.rs` | `Method`, `Shape` re-export |
| `crates/ulo-grpc/src/dispatch.rs` | the `:path` dispatcher over `tonic::server::Grpc` (R29), UNIMPLEMENTED |
| `crates/ulo-grpc/src/server.rs` | `Server`: h2 over `ulo-net` (R28), `grpc-timeout`, limits, drain, health switch |
| `crates/ulo-grpc/src/status.rs` | kind to code, `grpc-status-details-bin` (R32), the HTTP mapping (R31) |
| `crates/ulo-grpc/src/extract.rs` | `Message<T>`, `Request<T>`, `Streaming<T>`, `GrpcMetadata` |
| `crates/ulo-grpc/src/pre_dispatch.rs` | the gRPC stage (Q14) |
| `crates/ulo-grpc/src/health.rs`, `reflection.rs` | `GrpcHealth`, reflection registration |
| `crates/ulo-grpc/src/client.rs` | `GrpcClientModule`, `GrpcEndpoint`, `ClientTls`, `outgoing` |
| `crates/ulo-grpc-macros/src/lib.rs`, `method.rs` | `#[method(Marker)]` |
| `crates/ulo-grpc/src/__private.rs` | `GrpcHandler` |
| `crates/ulo-build/src/lib.rs`, `markers.rs` | `fw_grpc_build::configure().compile(..)`, the wrapping `ServiceGenerator` (R30) |

Design sections: §6, §10 (gRPC column), §12 rows; this review's R28–R34, Q11–Q14.

### Q. GraphQL

| File | Holds |
| --- | --- |
| `crates/ulo-graphql/src/lib.rs` | `Engine`, `GqlRequest`, `GqlResponse` with `outcome` (R39) |
| `crates/ulo-graphql-http/src/lib.rs`, `controller.rs`, `module.rs`, `playground.rs` | the controller, `GraphqlModule`, `GraphqlConfig`, the status rules (R36), the playground (R38) |
| `crates/ulo-graphql-ws/src/lib.rs`, `gateway.rs` | the graphql-transport-ws `Gateway` (R37) |
| `crates/ulo-graphql-async-graphql/src/lib.rs` | `AsyncGraphql<S, C>`, `dep` |
| `crates/ulo-graphql-juniper/src/lib.rs` | `Juniper<Q, M, Sub>`, `dep` |

Depends on W's `Gateway` trait (a hand-written gateway without macros) and on `ulo-http`'s
controller API. Design sections: §7; this review's R35–R39, Q15, Q16.

### E. The five embedding adapters and the HTTP conformance suite

| File | Holds |
| --- | --- |
| `crates/ulo-http-axum/src/lib.rs`, `layer.rs`, `run.rs` | `Axum`, the `ConnInfo` and `OriginalPath` layer, `NestedPath` check, `run` |
| `crates/ulo-http-salvo/src/lib.rs`, `handler.rs`, `run.rs` | `Salvo`, the `Handler`, prefix strip (R2), `run` |
| `crates/ulo-http-poem/src/lib.rs`, `endpoint.rs`, `run.rs` | `Poem`, the `Endpoint`, `take_upgrade`, `run` |
| `crates/ulo-http-actix/src/lib.rs`, `service.rs`, `pump.rs`, `run.rs` | `Actix`, `scope`, the payload pump, the connection-type flag, `run` |
| `crates/ulo-http-rocket/src/lib.rs`, `handler.rs`, `convert.rs`, `upgrade.rs`, `run.rs` | `Rocket`, the catch-all routes, http 0.2 conversion, buffered body (Q3), `Forward`, `IoHandler`, the cache `Routing`, `run` |
| `crates/ulo-http-conformance/src/lib.rs`, `cases/*.rs` | `Host`, the hyper reference, the case list of R4 |

Design sections: §3.8, §10 (embedded column), §12 rows; this review's R1–R4, Q1–Q3.

### D. The development command

| File | Holds |
| --- | --- |
| `crates/ulo-cli/src/commands/dev.rs`, `exec.rs` | watch, rebuild, restart, the held socket, the `__exec` trampoline (R41, R42), `ULO_DEV` (Q17) |
| `crates/ulo-net/src/endpoint.rs` | the `ULO_DEV` resolution of an `Addr` to an inherited name, if Q17 takes it |

Design sections: §9; this review's R40–R42, Q17, Q18.

### Cross-area contracts

Each row is an item one area calls in another; the owner keeps it as the spine writes it.

#### Owned by S

| Item | Called by | For |
| --- | --- | --- |
| `Inputs::also_seeded_by::<U>()`, the merge rule | W | `SessionHandle`, `ConnectionInfo`, `UpgradeHead` under `Ws` and `WsConnect` |
| `AppHandle::mounted::<T>()`, `Mounted::handlers_of::<U>()` | W, Q | the hand-off reading `MountedHandler<Ws>`; the standalone server reading `WsConnect` |
| `UpgradeHandler::{paths, upgrade, prepare, bound, drain, close}` | W | the gateway hand-off on the HTTP port and on an embedding |
| `ulo_transport::prepare::{Failures, Failure, Names, zero_bound, zero_count}` | W, R, G, Q, E | every `prepare` failure |
| `PreDispatch<T>` (if Q14) | G | the gRPC stage |
| `Request`, `Response`, `HttpBody`, `ConnInfo`, `OnUpgrade`, `Upgraded`, `Embed`, `Embedded`, `Handle`, `Service::respond`, `Routing`, `Forwardable`, `OriginalPath`, `EmbedLimits` | E | as built in 2a, unchanged |
| `ModuleDef::controller::<C>().at(prefix)`, `PreDispatch`, `Host<T>` | Q | the GraphQL controller's path and its reads |

#### Owned by W

| Item | Called by | For |
| --- | --- | --- |
| `Gateway` (the trait a hand-written gateway implements), `GatewayConfig`, `Session<T>`, `ConnectRefused`, `Frame`, `Reply` | Q | the graphql-transport-ws gateway |
| the `subprotocols` setting and its echo | Q | `graphql-transport-ws` in the 101 |

#### Owned by R

| Item | Called by | For |
| --- | --- | --- |
| `Link`, `Inbound`, `Outbound`, `Delivery`, `ReplyPath`, `Ack`, `Capabilities`, `Frame`, the codec trait | B | the seven links and the conformance suite |
| `RpcClient`, `RpcError` | B (the suite) | the caller's side of every case |

#### Owned by G

| Item | Called by | For |
| --- | --- | --- |
| `Method` | the build step's generated markers | `PATH`, `SHAPE`, `Request`, `Response` |

#### Owned by E

Nothing outward: every adapter consumes S's embedding surface and exports its aliases.

### Points each area resolves in its own log

- **W.** The `DisconnectReason` mapping from tungstenite's errors (Q5); whether the standalone
  server shares `ulo-http-hyper`'s accept loop or copies it (R17, pending R28's factoring).
- **R.** The `credit` reservation's exact fields; the TCP length prefix and `max_frame` default.
- **B.** The AMQP prefetch default (R24); the NATS and MQTT group name (Q10); Kafka topic creation
  as a setting.
- **G.** Whether `Detail::Json` packs as `google.protobuf.Value` or `google.protobuf.Struct` for an
  object (R32 says `Value`, always).
- **Q.** The `Accept` negotiation's default when the header is absent (R36).
- **E.** salvo's `HostRequest<'r>` (Q1); the rocket cap (Q3); the per-host `run` type parameters
  (R3).
- **D.** The `ULO_DEV` resolution rule (Q17) and what `--listen` accepts.
