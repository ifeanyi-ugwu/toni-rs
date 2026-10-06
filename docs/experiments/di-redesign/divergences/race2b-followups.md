# Divergences: race 2b's signed-off follow-ups

The build of the seven items `transports/RESPONSE.md` "Twenty-first response" accepted from
`transports/DIVERGENCES_2B.md`: U2, U5, U26, U33, U14, U24 and U3. Each section gives what was
written, then the decisions the response and the design left open. One defect outside the seven,
found while checking U2 against the RPC conformance suite, is fixed and recorded under U2.

Files changed: `Cargo.toml`, `Cargo.lock`, `crates/ulo/src/{transport/mod.rs, transport/controller.rs,
transport/handler.rs, graph/wire.rs, error/wiring.rs}`, `crates/ulo-transport/src/error.rs`,
`crates/ulo-handler-codegen/src/paths.rs`, `crates/ulo-net/src/activation.rs`,
`crates/ulo-http/src/{lib.rs, miss.rs, service.rs, transport.rs}`,
`crates/ulo-http-conformance/src/cases/disconnect.rs`, `crates/ulo-ws/src/transport.rs`,
`crates/ulo-rpc/src/{__private.rs, dispatch.rs, link.rs, server.rs}`,
`crates/ulo-rpc-macros/src/message.rs`, `crates/ulo-rpc-rabbitmq/{Cargo.toml, src/lib.rs,
src/link.rs}`, `crates/ulo-grpc/src/{dispatch.rs, server.rs, status.rs}`,
`crates/ulo-graphql/src/lib.rs`, `crates/ulo-graphql-http/src/{lib.rs, controller.rs, module.rs}`,
`crates/ulo-graphql-ws/src/gateway.rs`, `crates/ulo-graphql-async-graphql/src/lib.rs`,
`crates/ulo-graphql-juniper/src/lib.rs`. Deleted: `crates/ulo-ws-tungstenite/`.

## The signatures

```rust
// ulo (core)
pub trait Transport: 'static {
    const KEY: &'static str;
    const READS_PREFIX: bool = false;                       // new
    /* .. */
}
impl ControllerHandle<'_> { #[track_caller] pub fn at(self, prefix: impl Into<Cow<'static, str>>); } // now track_caller
pub enum WiringError {
    /* .. */
    UnreadPrefix { controller: KeyName, module: ModuleName, prefix: String, transports: Vec<TypeName>, at: &'static Location<'static> }, // new
}

// ulo-rpc
pub trait Link { /* .. */ fn max_inflight(&mut self, calls: Count) {} }   // new, defaulted
impl<L: Link> Server<L> { pub fn timeout_grace(self, grace: Bound) -> Self; } // new
pub mod __private {                                                        // new items
    pub struct ReplyShapeProbe<R>;
    impl<R> ReplyShapeProbe<R> { pub fn of<F: Fn() -> Fut, Fut: Future<Output = R>>(_call: &F) -> Self; }
    pub struct StreamedReplyOnEvent;
    pub trait StreamsInResult; pub trait StreamsBare; pub trait StreamsNot;
}

// ulo-rpc-rabbitmq
// RabbitMq::prefetch(u16) removed; `Link::max_inflight` sets the prefetch.

// ulo-grpc
impl Server { pub fn timeout_grace(self, grace: Bound) -> Self; }         // new

// ulo-http
impl MethodNotAllowed { pub fn new(method: Method, allow: HeaderValue) -> Self; } // was pub(crate)

// ulo-graphql
pub enum Outcome { Executed, RequestError, Failed }                       // `Failed` new
impl GqlResponse { pub fn failed(errors: Vec<GqlError>) -> Self; }        // new

// ulo-graphql-http
impl<Q> GraphqlConfig<Q> { pub fn engine_from<M: Module>(self, module: M) -> Self; } // new
```

## U2: a passed deadline on RPC and gRPC runs the error handlers under `timeout_grace`

**Written.** `ulo_rpc::Server::timeout_grace(Bound)` and `ulo_grpc::Server::timeout_grace(Bound)`,
one second at `Bound::Default` as HTTP's, `Bound::Unbounded` waiting for the handlers, a zero
refused in `prepare` through `zero_bound` with the `Bound::Unbounded` hint. When the deadline
passes before the call answers, the pipeline is dropped, the execution cancelled with `Deadline`,
and `ulo::recover` offers the error handlers a `CallError` of kind `Timeout`, "the call's deadline
passed", under the grace. What they answer is the reply; an error they return is rendered as it
stands, through `CallError::from_boxed` on RPC and through a new `status::render_as_is` on gRPC
(a `tonic::Status` as built, any other error by its kind). Unclaimed, the offered `Timeout`
renders as `err` of kind `timeout` and as DEADLINE_EXCEEDED. When the grace runs out first,
the recovery is dropped and the same two answers are sent.

### 1. The grace covers a deadline before the reply begins, not one during a streamed reply

- **Written:** a deadline passing while a streamed reply is written ends it as before: `err` of
  kind `timeout` on RPC, DEADLINE_EXCEEDED trailers on gRPC, offered to no error handler.
- **Why:** HTTP's T8 rule covers the route timeout before the answer; after it, `TimedBody`
  cancels and leaves the body to end. Once a stream has begun no handler can change the status,
  which is what the late path (`dispatch_late`) exists for, and an item's error already goes
  through it.

### 2. gRPC: the deadline covers the pre-dispatch stage, so the handlers offered depend on routing

- **Written:** the gRPC race wraps the unscoped stage, routing, the scoped stage and `dispatch`.
  Routing records the handler it matched in a `OnceLock` the call holds; a deadline passing
  after routing offers that handler's tiers then the global ones, and one passing before routing
  the global ones alone (`recover(None, ..)`). The context is rebuilt from the path, metadata and
  peer the call arrived with; it carries no request.
- **Why:** HTTP's route timeout starts after routing, so it always has a handler. gRPC's
  deadline is the caller's and starts when the call arrives.

### 3. An RPC event's passed deadline runs the handlers too

- **Written:** an `#[event]` whose `deadline-ms` passes is offered to the error handlers under the
  grace. An `Ok` from them acknowledges the event; an error, or the grace running out, rejects it
  without requeue, logged at `warn` for the error.
- **Why:** one rule for every call. The acknowledgment follows what the handlers answer, as it does
  for an event handler's own error that an error handler claims inside `dispatch`. A request sent
  to an event handler and claimed this way is answered with an empty `res`, as a completed one is.

### 4. A stream answered by the error handlers after the deadline

- **Written:** on RPC it ends at its first poll with `err` of kind `timeout`, since the execution
  is already cancelled and the reply loop races the cancellation. On gRPC it is written as the
  error handlers returned it.
- **Why:** each transport's existing streaming path decides it; neither adds a branch for an
  answer no realistic error handler gives. Flagged for sign-off as an inconsistency left open.

### 5. Found and fixed: the pipeline was dropped before the execution was cancelled

- **Found:** `ulo_rpc_conformance::cases::deadlines::deadline_ms`, run over TCP from a scratch
  crate, received `timeout` and then failed its second assertion: the stalled handler's future,
  dropped at the deadline, read `cancel_reason() == None`, not `Some(Deadline)`. The same scenario
  run against `HEAD`'s sources fails the same way. `tokio::select!` in RPC's `until_ended` drops
  the losing futures before the winning branch's body runs, and the body was where
  `cancel_with(Deadline)` sat; gRPC's and HTTP's `race` return `Expired` and drop the pipeline
  before their callers cancel.
- **Written:** RPC cancels inside the expiry branch's own future; gRPC's and HTTP's `race` take the
  execution and cancel before returning `Expired`. A handler's future dropped at the deadline now
  reads `Deadline`, the rule §2.6 and the conformance suite state. HTTP's `service.rs` changed
  for this alone.

## U5: the AMQP prefetch follows `max_inflight`

**Written.** `Link::max_inflight(&mut self, calls: Count)`, defaulted to doing nothing, called by
`Server::prepare` right after `Link::prepare`. The RabbitMQ link implements it; `RabbitMq::prefetch`
is removed, and the `Server::max_inflight` doc that already promised the tie is now true.

### 1. `Count::Max(n)` above 65 535

- **Written:** the prefetch is `u16::try_from(n).unwrap_or(u16::MAX)`; `Default` and `Unlimited`
  give 64.
- **Why:** `basic.qos` carries a 16-bit prefetch. AMQP's 0 means unlimited, which would hand one
  instance the whole queue, so the nearest bound below a larger `n` is 65 535, not 0.

### 2. `ulo-rpc-rabbitmq` depends on `ulo-transport`

- **Written:** a path dependency, for `Count` in the trait method's signature.
- **Why:** `ulo-rpc` re-exports no `ulo-transport` type, and an application already names `Count`
  from `ulo-transport`. A link crate implementing `max_inflight` needs the same dependency.

## U26: `Outcome::Failed`, rendered 500

**Written.** `Outcome::Failed` and `GqlResponse::failed(errors)`. Both engine adapters answer a
context that cannot be built with it, "the request's context could not be built", as before but
`Failed` in place of `RequestError`. `ulo-graphql-http` renders it 500; graphql-transport-ws
writes it as `error`, as it writes a request error.

### 1. 500 under both media types

- **Written:** `Failed` is 500 under `application/graphql-response+json` and under the legacy
  `application/json`.
- **Why:** the legacy media type's 200-for-everything rule covers GraphQL errors, request and
  field errors alike. A server fault is not a GraphQL error, and a 200 would tell a client its
  document ran.

### 2. juniper's unbuilt root is `Failed` too

- **Written:** `unbuilt()`, the answer for a root node `construct` did not store, is `Failed`.
- **Why:** it reports a fault in the server, which is what `Failed` names. It cannot be reached.

### 3. Where the authorization sentence went

- **Written:** on `GraphqlWs`'s doc in `ulo-graphql-ws`: "The connect guards are
  graphql-transport-ws's only authorization point: they run once per connection, and no `Ws`
  guard, interceptor or error handler runs per `subscribe`, the gateway being hand-written with no
  message handler for them to wrap."
- **Why:** that is where the gateway is documented. `ulo-ws` documents gateways in general,
  whose `#[message]` handlers do run the `Ws` tiers per message, so the sentence would be false
  there.

## U33: four requests and one retirement

**Written.**
- `MethodNotAllowed::new` is public, documented for a handler refusing a method on a path it
  answers. The GraphQL GET-mutation refusal returns `Err(CallError::new(BadRequest, "a mutation is
  not executed over GET; send it as a POST").with_source(MethodNotAllowed::new(GET, "GET, HEAD,
  POST, OPTIONS")))`. The error handlers receive it; unclaimed, `render_as_is` sends 405 with
  `Allow`.
- `crates/ulo-ws-tungstenite` is deleted, with its `exclude` entry; `watchexec-events` is dropped
  from the workspace dependencies, no member naming it.
- `Paths::transport` documents what the path supplies (`Param`, `controller`, `IntoReplyProbe` and
  its three arms) and how HTTP, RPC and WebSocket point it.
- `Activation::get` names its two callers, a server's `prepare` for an `Endpoint::Inherited` and
  `EndpointSpec::resolve` for an `Endpoint::Addr` under `ULO_DEV=1`, which reads `PidMismatch` as no
  socket held.
- `CallError::with_grpc_code` documents the `tonic::Status` passthrough: the error handlers see
  the `Status`, and one left unclaimed is sent unchanged.

### 1. The 405 body is problem details, not a GraphQL response

- **Written:** the refusal renders as every other unclaimed 405, problem details with `Allow`; the
  message is the `CallError`'s, the source carrying only the method and `Allow`.
- **Why:** one rule for a 405. `MethodNotAllowed`'s own text, "this path does not answer `GET`; it
  answers GET, HEAD, POST, OPTIONS", contradicts itself for a path that does answer GET, so the
  `CallError` carries the message. An error handler that wants the GraphQL body builds it.

### 2. References to the deleted crate outside the workspace

- **Left:** `integration-tests/Cargo.toml`, `examples/README.md` and `.github/workflows/ci.yml`
  still name `ulo-ws-tungstenite`. The first two are excluded and unported; the CI job also names
  `ulo-config` and `ulo-health`, excluded too, so it is stale on this branch as a whole.

## U14: the reply shape read from the output type

**Written.** `#[ulo_rpc::message]` and `#[ulo_rpc::event]` no longer read `Stream`, `TryStream`,
`BoxStream` or `LocalBoxStream` out of the return type as written. The mount function holds a
closure that is never called, whose future calls the handler with the receiver and each parameter
bound to `unreachable!()`; `ReplyShapeProbe::of(&closure)` names the future's output `R`, and three
arms ranked by autoref decide: `Result<S, E>` with `S: Stream` on `&&ReplyShapeProbe`,
`S: Stream` on `&ReplyShapeProbe`, anything else on `ReplyShapeProbe`. The expansion, for
`async fn aliased(&self) -> Ticks`:

```rust
#[allow(unreachable_code)]
let __ulo_reply = ::ulo_rpc::__private::ReplyShapeProbe::of(&|| async move {
    let __ulo_this: &Self = ::core::unreachable!();
    Self::aliased(__ulo_this).await
});
::ulo_rpc::__private::shape(false, (&&&__ulo_reply).streams())
```

### 1. A stream reply on `#[event]` stays a compile error at the return type

- **The design fold reads:** with a type-level probe this can no longer be a compile error at the
  return type, and states it as refused in `prepare` with the existing non-unary-event text.
- **Written:** it is still a compile error at the return type, now from the type. For `#[event]`
  the shape block adds `let (): () = (&&&__ulo_reply).event_reply();` spanned at the return type;
  the two stream arms' `event_reply` answers `StreamedReplyOnEvent`, so a stream behind an alias
  fails with "expected `()`, found `StreamedReplyOnEvent`" at the alias, the one error the build
  prints. The `prepare` refusal stays for a handler mounted by hand with a streamed shape.
- **Why:** the X24 metadata probe already turns an autoref answer into E0308 this way, and a
  compile error points at the signature where `prepare`'s report cannot. The design fold's text
  needs amending to say both refuse it. For sign-off.

### 2. What else reads a return type's spelling

- **Checked:** `ulo-ws-macros` passes no shape; `ulo-grpc-macros` takes the path and shape from the
  method marker and checks the reply through its type-level probe; `ulo-http-macros` reads no
  return type; `ulo-handler-codegen::reply` reads opaque types only to append `use<..>`, which
  changes captures and decides no shape. `params::is_arc_of_self` reads the receiver's spelling,
  `Arc<Self>`, to choose between `&*this` and `Dep::into_arc(this)`; that is the receiver, not a
  shape, and an aliased `Arc` receiver is refused at the receiver with the existing message.
  Nothing changed outside `ulo-rpc-macros`.

### 3. Limits

- An output type the mount function cannot resolve to a stream through its bounds, such as a
  projection on a controller's type parameter, reads as unary. An opaque type, an alias and a
  boxed stream all resolve.
- The closure costs nothing at run time and adds one type-check of the call per handler.

## U24: `GraphqlConfig::engine_from(module)`

**Written.** `engine_from<M: Module>(self, module: M) -> Self`, kept across `.engine::<E>()`.
`GraphqlModule::register` imports the module before mounting its controllers, so the engine's
module exports `dyn Engine` and needs no `global`. The docs of `ulo-graphql-http`, `GraphqlModule`
and `GraphqlWs` show:

```rust
#[module(providers = [/* schema, */ AsyncGraphql<ApiSchema, GqlContext> as dyn Engine], exports = [dyn Engine])]
pub struct ApiSchemaModule;

#[module(imports = [
    WsModule::for_root(),
    GraphqlModule::for_root(GraphqlConfig::at("/graphql").subscriptions("/graphql/ws").engine_from(ApiSchemaModule)),
])]
pub struct AppModule;
```

### 1. How the module is held

- **Written:** a private `EngineModule { identity: ModuleIdentity, module: Arc<dyn Module> }`, the
  identity read once in `engine_from`. `EngineModule` implements `Module` by delegation, its
  `identity()` the captured one and its `register` the module's, and `GraphqlModule` imports a
  clone of it. `GraphqlConfig`'s `Clone` clones the `Arc`; its `PartialEq` and `Hash` add the
  captured identity to the fields they already compare.
- **Why:** `ModuleIdentity` is already `Clone + Eq + Hash` and compares a configured module by
  value, so two configs naming equal modules are equal and `GraphqlModule`'s identity by value
  holds. The delegating identity makes the import the same instance as the module imported
  anywhere else in the application. `Arc` because `dyn Module` is not `Clone` and the config is.

### 2. The engine's module cannot import the `GraphqlModule` naming it

- **Written:** nothing prevents it; the wiring reports the import cycle. The examples import both
  from a third module.

## U3: a prefix no handler reads is refused

**Written.** `Transport::READS_PREFIX: bool`, `false` by default; `Http`, `Ws` and `WsConnect`
set `true`, `Rpc` and `Grpc` keep the default. Each `HandlerDecl` records its transport's value.
At the freeze, a controller registered with `.at(prefix)` none of whose handlers reads a prefix is
reported as `WiringError::UnreadPrefix` among step 2's errors, at `wire()`:

```text
  × the prefix `/api` on `RpcOnly` in AppModule is read by none of its handlers
    ├─ its handlers belong to Rpc, which join no prefix to their routes
    ├─ set by `.at(..)` at src/bin/prefix.rs:52
    └─ help: remove the `.at(..)`; a prefix reaches HTTP routes and WebSocket gateway paths
```

### 1. The default is `false`

- **Written:** a transport answers `true` only by declaring it.
- **Why:** the refusal exists for the silent no-op. With `true` as the default, a transport that
  joins no prefix and does not know the constant would ignore one silently; with `false`, a
  transport that joins prefixes and forgets the constant gets a refusal at `wire()` the first time
  someone sets one, which names the controller and the transport.

### 2. `Ws` answers `true`

- **Written:** both WebSocket transports answer `true`.
- **Why:** a `#[message]` handler's events are not prefixed, but every controller with one also
  mounts its gateway's connect handler (`WsConnect`), whose path is. `Ws` answering `true` keeps a
  `Ws`-only declaration list, which a hand-written mount could produce, from being refused.

### 3. A controller with no handler and a prefix is refused

- **Written:** "none of its handlers reads it" includes a controller with none; the second line
  reads "it mounts no handler".
- **Why:** nothing reads the prefix there either. A controller whose handlers all sit behind a
  `cfg` that is off is refused in that build only, as its routes are absent in that build only.

### 4. Reported at `wire()`, not `prepare`

- **Written:** a wiring error, with the `.at(..)` call's location; `.at` is `#[track_caller]` for it.
- **Why:** the freeze sees every controller's handlers with their transports before any server's
  `prepare`, and the report is one pass earlier, as U4 settled for module settings.

## Also fixed

- `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps` failed at `HEAD` in two files no
  item touched: `ulo-http`'s `stage` module doc linked `Stage`, `ScopedStage` and `StageHost` by a
  path not in scope, now `stage::Stage` and the like; `ulo-http-conformance`'s `mid_stream` linked
  the private `IDLE`, now written without a link.

## Verification

- `cargo check --workspace --all-targets` on stable: passes; its JSON warning list equals `HEAD`'s,
  the same 17 warnings, all in `crates/ulo/src` (`private_interfaces` in `binding/handle.rs`, dead
  code in `dependency/`, `graph/`, `lifecycle/`, `module/`, `redact.rs`, `timer.rs`).
- `cargo +1.88 check --workspace --all-targets --exclude ulo-http-salvo --exclude
  ulo-graphql-async-graphql`: passes.
- `cargo test --workspace`: passes. It runs no scenario touching these items: the transport suites
  are not stamped into tests on this branch.
- `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps`: passes, after the two fixes above.

Scratch crate `probe2b` (own `[workspace]`, path dependencies on the workspace crates, `HEAD`'s
`Cargo.lock`), run on rustc stable and on 1.88 with identical output:

- **U14, `shapes`** (`#![deny(warnings)]`): eight handlers, their mounted shapes read from
  `AppHandle::handlers()`, then the app bound to UDP, which carries no streamed shape.

  | Handler | Return type | Shape | Shape with `HEAD`'s macro |
  | --- | --- | --- | --- |
  | `aliased` | `Ticks` (`type Ticks = BoxStream<'static, Result<u32, CallError>>`) | ServerStreaming | Unary |
  | `opaque` | `impl Stream<Item = Result<u32, CallError>>` (async fn) | ServerStreaming | ServerStreaming |
  | `opaque_sync` | the same, non-async fn | ServerStreaming | ServerStreaming |
  | `fallible_alias` | `MaybeTicks` (`Result<Ticks, CallError>`) | ServerStreaming | Unary |
  | `arc_alias` | `Ticks`, `self: Arc<Self>` | ServerStreaming | Unary |
  | `unary_alias` | `Answer` (`Result<u32, CallError>`) | Unary | Unary |
  | `unary` | `u32` | Unary | Unary |
  | `event_alias` (`#[event]`) | `Answer` | Unary | Unary |

  On UDP the build refuses all five streaming handlers in `prepare`; with `HEAD`'s macro
  (`git show HEAD:crates/ulo-rpc-macros/src/message.rs`, restored afterwards and compared with
  `cmp`) it refuses only `opaque` and `opaque_sync`, and the three aliased streams would have
  answered `err` of kind `internal` at run time.
- **U14, `event_stream`:** `#[ulo_rpc::event] async fn streamed(&self) -> Ticks` fails with one
  error, E0308 "expected `()`, found `StreamedReplyOnEvent`", spanned at `Ticks`.
- **U3, `prefix`** (`#![deny(warnings)]`): wired four roots. `RpcOnly` (one `#[ulo_rpc::message]`)
  with `.at("/api")` is refused with the report above; `Mixed` (one `#[ulo_http::get]` and one
  `#[ulo_rpc::message]`) with `.at("/api")` wires; `HttpOnly` with `.at("/api")` wires; `RpcOnly`
  without a prefix wires.
- **U2, `deadline_tcp`:** `ulo_rpc_conformance::cases::deadlines::deadline_ms` over a TCP `Broker`
  written in the scratch crate. Against `HEAD`'s sources (extracted with `git archive`) it fails at
  `deadlines.rs:19`, `left: Some(None)`, `right: Some(Some(Deadline))`; with this build it passes:
  `timeout` on the wire through the grace path, unclaimed, and `Deadline` on the stalled handler's
  execution.

Not run: a claimed `Timeout` on RPC or gRPC, gRPC's deadline at all, and HTTP's route timeout after
the `race` change; nothing in the workspace or the scratch crate drives a gRPC server or an HTTP
route timeout.
