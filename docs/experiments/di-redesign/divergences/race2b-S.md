# Divergences: the race 2b spine

Every place the race 2b spine departs from `transports/DESIGN.md`, `DESIGN.md`, the twentieth
response or `REVIEW_2B.md`, or fills a shape they leave open. Each entry gives what the design
says, what the spine writes, and why. All await the user's sign-off.

`BUILD_PLAN_2B.md` names this log `divergences/race2b-spine.md`; the brief that launched the spine
names it `race2b-S.md`, the form every area's log takes, and this file follows the brief.

No cargo was run. Every claim about what compiles is read from the sources and the registry, and
the items a compile would have to show are listed under "Not verified" at the end.

## The frozen contracts, as written

```rust
// ulo (core)
impl Inputs {
    pub fn input<T: Send + Sync + 'static>(&mut self) -> &mut Self;
    pub fn also_seeded_by<U: Transport>(&mut self) -> &mut Self;        // X19, the last `input` written
}
impl<T: Transport> Mounted<'_, T> {
    pub fn handlers_of<U: Transport>(&self) -> Vec<MountedHandler<U>>;  // X20
}
impl AppHandle {
    pub fn mounted<T: Transport>(&self) -> Result<Vec<MountedHandler<T>>, TimerMissing>;  // X20
}
pub trait TransportMetadata: Send + Sync + 'static { type Transport: Transport; }          // X24
pub mod __private {
    pub struct MetaProbe<T, V: ?Sized>;   impl MetaProbe<T, V> { pub fn new(_: &V) -> Self; }
    pub struct MetaMismatch<T, U>;
    pub trait MetaSame { fn check(&self); }                       // on &&MetaProbe<T, V>, V::Transport == T
    pub trait MetaOther<T, U> { fn check(&self) -> MetaMismatch<T, U>; }   // on &MetaProbe<T, V>
    pub trait MetaAny { fn check(&self); }                        // on MetaProbe<T, V>
}

// ulo-handler-codegen
pub fn emit::metadata(meta: &MetaTokens, paths: &Paths) -> TokenStream;
// per #[meta] value, under its gates, spanned at it:
//   { let __ulo_value = <expr>;
//     let (): () = (&&&::ulo::__private::MetaProbe::<Marker, _>::new(&__ulo_value)).check();
//     __ulo_meta.<tier>(__ulo_value); }

// ulo-transport (X22)
pub mod prepare {
    #[derive(Default)] pub struct Failures(..);
    impl Failures {
        pub fn new() -> Self; pub fn push(&mut self, failure: impl Into<Failure>);
        pub fn push_error(&mut self, error: BoxError); pub fn is_empty(&self) -> bool; pub fn len(&self) -> usize;
        pub fn into_error(self) -> PrepareError; pub fn into_result(self) -> Result<(), BoxError>;
    }
    impl<F: Into<Failure>> Extend<F> for Failures;
    pub struct Failure(..);
    impl Failure {
        pub fn plain(text: impl Into<String>) -> Failure;
        pub fn naming(names: Vec<TypeName>, text: impl Fn(&Names<'_>) -> String + Send + Sync + 'static) -> Failure;
        pub fn from_error(error: BoxError) -> Failure; pub fn names(&self) -> &[TypeName];
        pub fn text(&self, names: &Names<'_>) -> String;
    }
    impl From<String> for Failure; impl From<&str> for Failure; impl From<PrepareError> for Failure;
    pub struct Names<'a>;  impl<'a> Names<'a> { pub fn new(full: &'a HashSet<TypeName>) -> Self; pub fn of(&self, name: TypeName) -> String; }
    pub fn zero_bound(setting: &str, bound: Bound, effect: &str) -> Option<Failure>;
    pub fn zero_count(setting: &str, count: Count, effect: &str) -> Option<Failure>;
}
pub const span::WS_EVENT: &str = "ws.event";     // and `ws.event` declared empty by `span::call`

// ulo-http
pub trait UpgradeHandler: Send + Sync + 'static {                                          // X21
    fn paths(&self, app: &AppHandle) -> Vec<Cow<'static, str>>;
    fn upgrade(&self, req: Request) -> BoxFuture<'static, Response>;
    fn prepare(&self, app: &AppHandle) -> Result<(), BoxError> { Ok(()) }
    fn bound(&self, app: &AppHandle) {}
    fn drain(&self, token: DrainToken) -> BoxFuture<'_, ()> { /* ready */ }
    fn close(&self) -> BoxFuture<'_, ()> { /* ready */ }
}
impl Upgrades { pub fn register(&mut self, handler: impl UpgradeHandler) -> &mut Self; }
pub trait HttpCarried: Transport + __private::Carried {}                                   // X23
impl HttpCarried for Http {}
pub mod __private { pub trait Carried {} }
pub struct PreDispatch<T: HttpCarried = Http> { .. }       // every method as in 2a, on impl<T: HttpCarried>
pub mod stage {
    pub type Rest = Box<dyn FnOnce(Request) -> BoxFuture<'static, Response> + Send>;
    pub trait StageHost: Send + Sync + 'static {
        fn app(&self) -> &AppHandle; fn exec(&self) -> &ExecutionRef;
        fn fail(&self, err: BoxError) -> BoxFuture<'_, Response>;
    }
    pub struct Stage<T: HttpCarried = Http>;
    impl<T: HttpCarried> Stage<T> {
        pub fn build(metas: Vec<(ModuleRef, Arc<PreDispatch<T>>)>) -> Result<Stage<T>, Vec<String>>;
        pub fn scoped(&self, route: &str) -> Result<Arc<ScopedStage<T>>, String>;
        pub fn run(&self, host: Arc<dyn StageHost>, req: Request, end: Rest) -> BoxFuture<'static, Response>;
        pub fn is_empty(&self) -> bool;
    }
    pub struct ScopedStage<T: HttpCarried = Http>;
    impl<T: HttpCarried> ScopedStage<T> {
        pub fn run(&self, host: Arc<dyn StageHost>, req: Request, end: Rest) -> BoxFuture<'static, Response>;
        pub fn is_empty(&self) -> bool;
    }
}
impl TransportMetadata for Timeout { type Transport = Http; }
impl TransportMetadata for BodyLimit { type Transport = Http; }

// ulo-hyper-serve
#[derive(Clone, Debug, Default)] pub struct ServeConfig { pub handshake_timeout: Option<Duration> }
pub struct Accepted { pub io: Io, pub conn: ConnInfo, pub draining: Draining }
#[derive(Clone)] pub struct Draining;  impl Draining { pub async fn wait(&mut self); pub fn is_draining(&self) -> bool; }
pub struct Io;  // tokio AsyncRead + AsyncWrite + Unpin;  impl Io { pub fn is_tls(&self) -> bool; }
pub struct Serve;
impl Serve {
    pub fn new(listeners: Vec<BoundListener>, tls: Option<TlsAcceptor>, config: &ServeConfig) -> io::Result<Serve>;
    pub async fn run<F, Fut>(&self, connection: F) -> Result<(), BoxError>
    where F: Fn(Accepted) -> Fut + Send + Sync + 'static, Fut: Future<Output = ()> + Send + 'static;
    pub async fn drain(&self);
    pub async fn close(&self);
}
```

The embedding surface E consumes, `ModuleDef::controller::<C>().at(prefix)`, `Host<T>`,
`Dep<RequestHead>` and `ulo_net`'s items are unchanged from race 2a.

The new crates' contracts other areas call, written by the spine for their owners to keep:

```rust
// ulo-ws, owned by W, called by Q
pub trait GatewayConfig: Construct {
    fn settings() -> GatewaySettings;
    fn connect_guards(spec: &mut EnhancerSpec<WsConnect>) {}
    fn session() -> SessionFactory { SessionFactory::none() }
    fn mount_gateway(m: &mut Mount<'_>);
}
pub trait Gateway: GatewayConfig {
    fn on_connect(&self, conn: &Connection) -> impl Future<Output = Result<(), ConnectRefused>> + Send { .. }
    fn on_message(&self, conn: &Connection, frame: Frame) -> impl Future<Output = ()> + Send;
    fn on_disconnect(&self, conn: &Connection, why: DisconnectReason) -> impl Future<Output = ()> + Send { .. }
    fn after_init(&self, gw: GatewayRef) -> impl Future<Output = ()> + Send { .. }
    fn mount(m: &mut Mount<'_>) { .. }
}
#[non_exhaustive] pub struct GatewaySettings { pub path, pub namespace, pub event, pub codec: Codec,
    pub subprotocols: Vec<Cow<'static, str>>, pub refuse: Refuse, pub message_limit: Option<u64>,
    pub max_connections: Count, pub max_inflight: Count, pub max_outbound: Count, pub overflow: Overflow,
    pub ping_interval: Bound, pub pong_timeout: Bound }     // GatewaySettings::at(path) and one setter each
pub enum Frame { Text(String), Binary(Bytes) }
pub enum Reply { None, One(Frame), Many(Tracked<BoxStream<'static, Result<Frame, BoxError>>>) }
impl ConnectRefused { pub fn code(code: u16, reason: impl Into<String>) -> Result<Self, CloseCodeError>;
    pub fn kind(kind: ErrorKind, reason: impl Into<String>) -> Self; pub fn close_code(&self) -> u16; pub fn reason(&self) -> &str; }
impl Connection { pub fn id(&self) -> ConnId; pub async fn join(..); pub async fn leave(..); pub fn rooms(..);
    pub async fn send(&self, frame: Frame) -> Result<(), BroadcastError>; pub async fn close(&self, code: u16, reason: &str);
    pub fn open_execution(&self) -> Result<Execution, Closed>; pub fn info(..); pub fn head(..); pub fn session(&self) -> &SessionHandle;
    pub fn timer(&self) -> &Arc<dyn Timer>; }
impl SessionHandle { pub fn get<T: Send + Sync + 'static>(&self) -> Option<Session<T>>; }
impl WsModule { pub fn for_root() -> Self; pub fn broadcast(self, adapter: impl BroadcastAdapter) -> Self; /* defaults */ }

// ulo-rpc, owned by R, called by B
pub mod link {
    pub trait Link: Send + Sync + 'static {
        const NAME: &'static str;
        fn capabilities(&self) -> Capabilities;
        fn prepare(&mut self, app: &AppHandle) -> impl Future<Output = Result<(), BoxError>> + Send { Ok(()) }
        fn listen(&self, patterns: &[Pattern]) -> impl Future<Output = Result<Inbound, BoxError>> + Send;
        fn connect(&self) -> impl Future<Output = Result<Outbound, BoxError>> + Send;
        fn drain(&self) -> impl Future<Output = ()> + Send;
        fn close(&self) -> impl Future<Output = Result<(), BoxError>> + Send;
        fn bound(&self) -> Vec<BoundAddr> { Vec::new() }
    }
    pub type Inbound = BoxStream<'static, Delivery>;
    pub struct Delivery { pub frame: Frame, pub reply: Option<ReplyPath>, pub ack: Ack }
    impl ReplyPath { pub fn new(send: impl Fn(Frame) -> BoxFuture<'static, Result<(), BoxError>> + ..) -> Self;
        pub fn send(&self, frame: Frame) -> BoxFuture<'static, Result<(), BoxError>>; }
    impl Ack { pub fn new(settle: impl FnOnce(bool) + Send + 'static) -> Self; pub fn none() -> Self; pub fn ack(self); pub fn reject(self); }
    pub struct Outbound { pub send: Box<dyn Fn(Pattern, Frame, Option<ReplyTo>) -> BoxFuture<'static, Result<(), BoxError>> + Send + Sync>,
                          pub replies: BoxStream<'static, Frame> }
    pub struct ReplyTo { pub id: u64 }
    pub struct Pattern(..);  // new, as_str, From<&str>, From<String>, Display
    #[non_exhaustive] pub struct Capabilities { pub binary, pub max_frame: Option<u64>, pub ordering: Ordering,
        pub shapes: &'static [Shape], pub native_backpressure, pub delivery: DeliveryMode, pub miss_signal }
    impl Capabilities { pub const fn new(delivery: DeliveryMode) -> Self; /* one const setter each */ }
    pub const ALL_SHAPES: &[Shape]; pub const UNARY_ONLY: &[Shape];
    pub enum DeliveryMode { Competing, FanOut, Addressed }
    pub enum Ordering { Unordered, PerConnection, PerPublisher, PerChannel, PerQueue, PerTopic, PerPartition }
    pub struct NoDestination { pub pattern: String }      // a link's miss signal, from `send`
    pub struct FrameTooLarge { pub size: u64, pub limit: u64 }
}
pub enum Frame { Req{id, pattern, headers, data}, Evt{..}, Res{id, data}, Err{id, error: ErrorBody}, Item{..},
                 End{id}, Open{..}, In{..}, InEnd{id}, Cancel{id}, Credit{id, n}, Goaway }
pub struct Data(..);  // encoded payload bytes: new, as_bytes, into_bytes
pub enum Codec { Json, Cbor }   // binary, encode_frame, decode_frame, encode, decode
pub enum PayloadKind { Serde, Binary }
impl RpcClient { request, emit, stream, send_stream, duplex }   // Call<Res>, Emit, StreamCall<Res>: IntoFuture
impl RpcError { pub fn kind(&self) -> ErrorKind; pub fn message(&self) -> &str; pub fn details(&self) -> &Details; pub fn reason(&self) -> Option<&str>; }
impl<L: Link> RpcClientModule<L> { pub fn for_root(link: L) -> Self; pub fn timeout(self, timeout: Bound) -> Self; }

// ulo-grpc, owned by G, called by ulo-build's generated code
pub trait Method: Send + Sync + 'static {
    const PATH: &'static str; const SHAPE: Shape;
    type Request: prost::Message + Default + Send + 'static;
    type Response: prost::Message + Default + Send + 'static;
}
pub trait GrpcClient: Send + Sync + 'static { fn from_channel(channel: Channel) -> Self; }
impl ulo_http::__private::Carried for Grpc {}  impl ulo_http::HttpCarried for Grpc {}
pub type PreDispatch = ulo_http::PreDispatch<Grpc>;
```

## Core

### 1. A module declaration naming one of a transport's seeders is that declaration

- **Design:** §2.10: a module's declaration and a transport's "with the same `(key, seeder)` is one
  declaration"; X19 gives a transport's declaration a seeder set and merges two transports' with
  equal sets.
- **Spine:** `InputDecl.seeders: Vec<TypeName>`, the declaring transport first. A module's
  `seeded_by::<Tr>()` is the same declaration when `Tr` is among the transport declaration's
  seeders, and an earlier module's entry in the graph is widened to the transport's full set, so
  a handler of every seeder passes the input check. A module's seeder outside the set stays
  `InputConflict`.
- **Why:** with a set, "the same seeder" has to mean membership; requiring the module to name the
  whole set has no spelling, since `Input::seeded_by` names one transport.

### 2. `also_seeded_by` with no `input` before it declares nothing

- **Design:** silent.
- **Spine:** a no-op.
- **Why:** the core may not panic, and no wiring error exists for it; a transport writes its own
  `inputs`, where the mistake has one author.

### 3. `InputNotSeeded` keeps one `seeder`

- **Design:** silent on the report for an input with several seeders.
- **Spine:** the public field `seeder: TypeName` is unchanged and carries the first seeder, the
  declaring transport. `graph/scopes.rs` changed with `InputDecl` though the plan does not list it.
- **Why:** widening the field to a list changes a public variant; a report naming the declaring
  transport is what §2.10 says a report names.

### 4. `handlers_of` answers an owned `Vec`

- **Design:** `Mounted::handlers_of::<U>()`, return type unwritten.
- **Spine:** `Vec<MountedHandler<U>>`; `handlers()` keeps its slice. `mounted_parts` now fails with
  `TimerMissing` rather than a `BoxError`, which `AppHandle::mounted` returns as it stands.
- **Why:** `Mounted` holds a borrow of `T`'s handlers only; `U`'s are built on request.

### 5. X24's probe: names, bounds and one evaluation of the value

- **Design:** `TransportMetadata { type Transport: Transport; }`; the probe
  `(&&&MetaProbe::<T, _>::new(&value)).check()` with three arms.
- **Spine:** `TransportMetadata: Send + Sync + 'static`, which every metadata value is. The arms
  are the traits `MetaSame` (`&&MetaProbe<T, V>` where `V: TransportMetadata<Transport = T>`),
  `MetaOther<T, U>` (`&MetaProbe<T, V>`, answering `MetaMismatch<T, V::Transport>`) and `MetaAny`
  (`MetaProbe<T, V>`), imported anonymously as the reply probe's are. The generated code binds the
  value to a local, probes the local, then records it, so an expression with side effects runs
  once.
- **Why:** writing `&<expr>` in the probe and `<expr>` in `meta.method(..)` would build every
  value twice.

### 6. `emit::metadata` takes the transport's paths

- **Design:** §1, the probe is "written once, in `fw-handler-codegen`".
- **Spine:** `pub fn metadata(meta: &MetaTokens, paths: &Paths)`; `MountFn::emit` passes its own.
- **Why:** the probe names the handler's marker type.

## `ulo-transport`

### 7. X22's shapes

- **Design:** `prepare::{Failures, Failure, Names, zero_bound(setting, bound, effect),
  zero_count(setting, count, effect)}`, moved from `ulo-http`.
- **Spine:** the signature block above. `Failure` became opaque, built with `plain`, `naming` or
  `from_error`. `push_error` and `Failure::from_error` keep a `PrepareError`'s names, so an
  upgrade handler's or a link's typed failure enters the report's naming pass. The two checks
  answer `Option<Failure>`, written `failures.extend(zero_count(..))`. `into_error` on no failure
  writes "prepare failed" where HTTP's wrote "the route table could not be built"; HTTP reaches
  that branch never, since a failed stage or route table always records a failure.
- **Why:** a transport other than HTTP needs to fold another component's `BoxError` in, and an
  enum another crate constructs by variant cannot gain a field.

### 8. `span::call` declares `ws.event`

- **Design:** §2.9 names `ws.event` for WebSocket; R48 says it "joins the list".
- **Spine:** `pub const WS_EVENT` and the field, declared empty. `span.rs` is in no area's list.
- **Why:** `span::call` declares every field up front, and W cannot edit it.

## `ulo-http`

### 9. X21's lifecycle, as called

- **Design:** `prepare(&self, app) -> Result<(), BoxError>`, `bound(&self, app)`, `drain(&self,
  token) -> BoxFuture<'_, ()>`, `close(&self) -> BoxFuture<'_, ()>`; `Server<B>` calls them, and
  `Embedded<A>`'s `drain` and `close` "as `Server<B>` does".
- **Spine:** `bound` returns nothing, and work that waits (a gateway's `AfterInit`) is the
  handler's to spawn. Every registered handler, deduplicated by `Arc` identity, runs `prepare`
  inside the shared `prepare_app`, so on a backend and on an embedding; `bound` after the
  backend binds and in `Embedded::bind`; `drain` and `close` joined with the backend's or the
  host's. `Embedded::bind` calling `bound` goes beyond the plan's row.
- **Why:** `AfterInit` runs "after `listen()`", which no core hook marks; a `bound` the server
  awaited would hold `listen()` on user code. A gateway on an embedding host has the same
  `AfterInit` as one on a backend.

### 10. How `HttpCarried` is sealed (the spine's open point)

- **Design:** a sealed subtrait of `Transport` that `Http` and `Grpc` implement.
- **Spine:** `pub trait HttpCarried: Transport + ulo_http::__private::Carried {}`; `Carried` is
  public and doc-hidden. `ulo-grpc` implements both for `Grpc`. An `on_unimplemented` names the
  two transports.
- **Why:** a `pub(crate)` seal would keep `ulo-grpc` out; the doc-hidden module is the workspace's
  existing boundary for what only generated or sibling code touches. It seals by convention, not
  by the compiler.

### 11. A public stage API for gRPC

- **Design:** §6.2, gRPC's stage is `PreDispatch<Grpc>` "as HTTP's does"; silent on how `ulo-grpc`
  runs entries whose types are `ulo-http`'s and crate-private.
- **Spine:** `ulo_http::stage::{Stage<T>, ScopedStage<T>, StageHost, Rest}` (signature block).
  HTTP's own run goes through the same `Stage<Http>` and a `StageHost` impl on its sub-step
  context; `PreDispatch<T>`'s `Default` is written by hand, since a derive would demand
  `T: Default`, and `adopt` and `supplies` name their value type `V`, the turbofish unchanged.
- **Why:** without it G would have to duplicate the stage, which Q14 ruled out.

### 12. `ulo-http-hyper` files outside the plan's row

- **Design:** the plan lists `backend.rs`, `listener.rs`, `Cargo.toml`.
- **Spine:** `listener.rs` is deleted, its contents in `ulo-hyper-serve`; `lib.rs` drops the module
  and `convert.rs` builds a request's `ConnInfo` from the accepted connection's.
- **Why:** both follow from moving the loop out.

## `ulo-hyper-serve`

### 13. `Serve`'s signatures and the accept loop's open points

- **Design:** `Serve::new(listeners, tls, &ServeConfig)`, `Serve::run(self, connection)`,
  `drain()`, `close()`; open: whether `Accepted` carries `Draining`, and the `JoinSet` shutdown
  order.
- **Spine:**
  - `new` answers `io::Result<Serve>`: tokio's `TcpListener::from_std` can fail, and must run
    inside the runtime, which a server's `bind` is.
  - `run` takes `&self`: the core calls a server's `serve`, `drain` and `close` on one `&self`,
    so the server holds one `Serve` across the four. A second `run`, or one after `drain` or
    `close`, returns at once.
  - `Accepted` carries the connection's `Draining`, its own type with `wait` and `is_draining`.
  - `conn` is `ulo_http::ConnInfo`, its `version` HTTP/2 where ALPN settled `h2` and HTTP/1.1
    otherwise; a consumer sets each request's version. `ulo-hyper-serve` depends on `ulo-http`
    for `ConnInfo` and `TlsInfo`.
  - `close` raises a signal each accept loop watches; each loop aborts its connections through
    `JoinSet::shutdown`, which awaits every abort, and returns; `run` then ends and marks the
    loop finished, on which `drain` and `close` wait. A `drain` or `close` before `run` releases
    the listeners and marks it finished.
  - `ServeConfig::default()` is no handshake clock; a server passes its resolved
    `handshake_timeout`.

## Workspace

### 14. Dependencies

- **Plan:** the new workspace dependencies it lists.
- **Spine:** all of them, plus `actix-http` and `actix-service` (the actix `run` names
  `IntoServiceFactory` and `Request`, which actix-web does not re-export), `prost-build` (the
  `ServiceGenerator` trait), `watchexec-events`, `-signals`, `-filterer-ignore` and
  `ignore-files`. `default-features = false` on `async-nats`, `salvo`, `poem`, `actix-web`,
  `actix-http`, `rocket`, `async-graphql`, `tonic` and the existing `tokio-tungstenite`, each
  member enabling what it uses; `redis` gains `aio` and `tokio-comp`.
- **Why:** a member cannot turn off a workspace dependency's defaults, and those defaults pull TLS
  backends, compression and runtimes an area may not want. `ciborium` and `rmp-serde` are not in
  the offline registry; the first compile fetches them.

### 15. What was removed from `master`'s crates

- **Plan:** the old `src/**` of each crate rewritten in place is removed.
- **Spine:** also each one's `tests/`, `examples/` and `README.md`, and `ulo-cli`'s
  `commands/dev.rs` body; `ulo-ws-tungstenite` stays excluded.
- **Why:** the old tests and examples call the old API and would fail `cargo test`, and a README
  describing it is wrong once the crate is rewritten. The coordinator writes the new tests.

## The new crates' surfaces (decisions the designs leave open)

### WebSocket

16. **`Gateway` and `GatewayConfig`.** Design: `Gateway` is "the trait a hand-written gateway
    implements", `GatewayConfig` what `#[gateway]` emits, with `mount_gateway(m)`. Spine:
    `GatewayConfig: Construct` with `settings`, `connect_guards`, `session` and `mount_gateway`;
    `Gateway: GatewayConfig` with `on_message` and defaulted `on_connect`, `on_disconnect`,
    `after_init` and `mount`. Why: an attributed gateway's hooks are found by probing the concrete
    type, which a generic default cannot do, so a hand-written gateway carries its hooks as
    methods.
17. **`GatewaySettings`**, `#[non_exhaustive]` with `at(path)` and a setter per field;
    `message_limit` is `Option<u64>`, `None` deferring to `Server` or `WsModule`. Why: a gateway's
    unset limit needs a third state, and `u64` has none.
18. **`Frame`** is one data message's content, `Text` or `Binary`: a reply's `data`, encoded by the
    gateway's codec, or, through `Connection::send`, a message as it stands. **`ConnectReply
    { Admitted, Refused(ConnectRefused) }`** is `WsConnect::Reply`, which the design calls "admit or
    refuse". **`ConnectRefused::kind(kind, reason)`** sits beside `code(..)`, with `close_code()` and
    `reason()` as accessors. **`MessageId`** keeps the id's JSON text.
19. **Rooms:** `ConnId { node, seq }` with `NodeId` per process; `Target { gateway, audience, except }`
    and `Audience { All, Room, Client }` for the adapter SPI; `Rooms::to_*` answer a `Broadcast`
    whose `emit` answers `Result<(), BroadcastError>`, classified `Unavailable`.
20. **`Connection`** carries what a hand-written gateway needs: `send`, `close`, `open_execution`
    (an execution with the connection's inputs seeded), `session`, `timer`.

### RPC

21. **`Reply` carries `Data`, not `Frame`.** Design §5.1: `None | One(Frame) | Many(..Frame..)`.
    Why: RPC's `Frame` is the grammar, and a handler's answer is a payload the dispatcher wraps in
    `res` or `item` with the call's id.
22. **`Codec` is an enum, `Json | Cbor`, not a trait.** The plan's contract table names "the codec
    trait". Why: the set is closed, and a trait with generic encode and decode methods would make
    every link generic over its codec.
23. **`Data`** is the payload encoded by the link's codec, as bytes, so a broker's request-lane body
    is a `Data` as it stands. **`ErrorBody { kind, message, details }`**; **`CallHeaders`** ordered
    pairs; **`Frame`** an enum of struct variants.
24. **`Inbound` twice.** Design: the link SPI's `type Inbound = BoxStream<'static, Delivery>` and the
    extractor `Inbound<T>`, both at the crate root, which cannot coexist. Spine: the SPI lives in the
    public module `ulo_rpc::link`, and the root re-exports all of it but the alias, so the root
    `Inbound<T>` is the extractor and links write `ulo_rpc::link::Inbound`.
25. **`Link::prepare(&mut self, app: &AppHandle)`.** Design: `prepare(&mut self)`. Why: NATS and MQTT
    default their group to the root module's full type path (twentieth response, R22), and the SPI
    otherwise gives a link no app. A client's link reports its parse failures from `connect`.
26. **`Capabilities`** is built with `Capabilities::new(delivery)` and `const` setters, since a
    `#[non_exhaustive]` struct cannot be built by literal in a link crate; `ALL_SHAPES` and
    `UNARY_ONLY` are constants. **`Ordering`**'s variants are §5.3's table.
27. **Added for B:** `NoDestination` and `FrameTooLarge`, the errors a link's `send` answers for a
    miss signal and an oversized frame, which `RpcClient` maps to `no_destination` and
    `payload_too_large`; `Pattern`; `ReplyTo { id }`. **`Ack`** dropped unsettled sends nothing, so
    the broker redelivers.
28. **Client:** `request`, `emit`, `stream`, `send_stream` and `duplex` answer the builders `Call<Res>`,
    `Emit` and `StreamCall<Res>`, each `IntoFuture`, the last yielding `RpcStream<Res>`;
    `RpcClientModule<L>` is generic over its link.
29. **Link names:** `Tcp::new`, `Udp::new`, `Nats::url`, `Redis::url`, `RabbitMq::url`, `Mqtt::url`
    (with `QoS`, `AtLeastOnce` unset), `Kafka::brokers`; `group` on NATS, MQTT and Kafka;
    `reply_topic` on Kafka. Kafka's topic-creation settings are B's to name.

### gRPC

30. **`Response<T>`**, the example's reply type with `.metadata(k, v)`, is in `transport.rs`; the
    plan's rows do not name it.
31. **`GrpcClient { fn from_channel(Channel) -> Self }`**, which `ulo-build` implements for every
    client it writes. Design: `GrpcClientModule::<C>::for_root(endpoint)`, silent on how `C` is
    built; tonic's clients share no constructor trait.
32. **`Server`** adds `handshake_timeout`, which the accept loop takes, and
    `file_descriptor_set(&'static [u8])`, through which reflection receives the generated set;
    `reflection(bool)` defaults to `cfg!(debug_assertions)`.
33. **`ClientTls::ca_pem`** beside `system_roots`, for a private CA. **`GrpcHealth`** has `set::<S>`
    and `set_named`. **`status`** exports `code_for`, `code_for_http` and `to_status`.
34. **`include_proto!`** includes `$OUT_DIR/<package>.rs`; the descriptor constant is for `ulo-build`
    to write into that file.
35. **`ulo-build`:** `configure() -> Builder` with `out_dir`, `file_descriptor_set_path`,
    `build_client` and `compile(protos, includes)`.

### GraphQL

36. `GqlRequest`'s `variables` and `extensions` are `Option<Map>`, so a JSON `null` reads; `GqlResponse`
    is built by `executed` and `request_error`; `GqlError`, `Location` and `PathSegment` follow the
    specification's response format.
37. `GraphqlConfig<Q = ()>` and `GraphqlModule<Q = ()>`, `engine::<E>()` changing the parameter. This
    answers Q's open point at the type level; Q may answer it otherwise and log it.
38. `ulo-graphql-ws` exports `GraphqlWs<Q = ()>`, a `Construct`, `Gateway` and `Controller`, and
    `ConnectionInit`, the session type holding the init payload.
39. `ulo-graphql-async-graphql` implements `Engine` for `AsyncGraphql<Schema<Q, M, Sub>, C>` and
    declares MSRV 1.89, `ulo-http-salvo` 1.92, as the workspace records.

### Embedding and the two suites

40. **Adapter entry points:** `handler(&Handle) -> SalvoHandler`, `endpoint(&Handle) -> PoemEndpoint`,
    `scope(path, &Handle) -> ActixScope`, `routes(&Handle) -> Vec<rocket::Route>`, and axum's
    `HostLayer`/`HostService` for the `ConnInfo` and `OriginalPath` insertion.
41. **`run`** answers `Result<Shutdown, BoxError>`, its type parameters carrying only the bounds the
    host struct itself declares (actix's `HttpServer` where-clauses repeated). E adds the bounds
    each body needs, the open point the plan gives E.
42. **`Host::start(app: App<Connected>, mode)`**. Design: `start(app_service, mode)`. Why: an adapter
    host binds its own `Embedded<A>`, which takes the app before `listen()`, not an `AppService`.
    The suite exports `app()`, `Mode`, `PREFIX` and `HyperHost`; the rocket fairing sits behind the
    `rocket` feature.
43. **Both suites' macros** stamp `#[::tokio::test]`, so the invoking crate depends on tokio:
    `#[tokio::test(crate = ..)]` takes a string, where `$crate` is not substituted.

### `ulo-cli`

44. `commands/mod.rs` gains `pub mod exec;` under the `dev` feature; no area's row lists it.
    `DevArgs` keeps `master`'s flags, the command's surface; `place_listen_fd` is carried over with
    its body. `master`'s `socket_handoff` tests for it are not carried, under rule 2; D brings them
    back after the race.

## Requests and unsettled points for the areas

- **W:** which gateways the standalone `Server` serves and which the hand-off serves is in neither
  design; a gateway on both ports needs a selector. `IntoReply<Ws>` is not written: the four impls
  §4.1 lists overlap under coherence (`()` is `Serialize`, and a type could be both `Stream` and
  `Serialize`, E0119), so W picks the mechanism.
- **R:** the same holds for `IntoReply<Rpc>` and `T: Serialize`.
- **G:** `CallError::grpc_code(..)` (decision 2) belongs on `CallError` in `ulo-transport`, which no
  area owns; it needs routing to an owner.
- **Q:** `IntoReply<Http> for GqlResponse` cannot be written in `ulo-graphql-http`: the trait is
  `ulo-transport`'s, the type `ulo-graphql`'s and the marker `ulo-http`'s, which the orphan rule
  refuses. A local wrapper or a reply built in the controller is Q's to choose. Who mounts the
  graphql-transport-ws gateway for `GraphqlConfig::subscriptions(path)` is open too, since §1 has
  `ulo-graphql-http` meet the core through `ulo-http` alone.
- **E:** see entry 41.
- **D:** entry 44.

## Not verified

- Nothing was compiled. In particular: method probing rejecting `MetaSame` through the projection
  bound `TransportMetadata<Transport = T>` rather than reporting a mismatch (the reply probe's arms
  use trait bounds alone); the juniper bounds on `Juniper<Q, M, Sub>` and the `Dep<RootNode<..>>`
  read, which need `RootNode: Send + Sync`; `tonic::Streaming<T>` being `Unpin` for the extractor's
  `poll_next`.
- Each macro crate's attribute is `todo!()`, so until W, R and G fill them any crate writing
  `#[ulo_ws::message]`, `#[ulo_rpc::message]` or `#[ulo_grpc::method]` fails at expansion. The core,
  `ulo-http`, `ulo-http-hyper` and `ulo-hyper-serve` carry no `todo!()`.
