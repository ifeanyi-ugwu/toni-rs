# Build plan: filling in the race 2a spine

Race 2a builds the transport layer's first stage: the core extensions X1–X9 and X11–X15, the
shared transport crates, `ulo-tokio`, `ulo-http` and the axum backend. WebSocket, RPC, gRPC,
GraphQL, the other four HTTP backends and the dev command are race 2b.

The spine is every crate's module tree, every public signature and the `pub(crate)` structures
that cross area boundaries, with `todo!()` bodies. It was written without compiling. Seven areas
fill it in parallel; each owns its files and builds against the signatures the others carry.

`transports/DESIGN.md` and `DESIGN.md` are the spec; `transports/RESPONSE.md` breaks a tie
between them, the latest response winning. `divergences/race2a-spine.md` lists every place the
spine departs from the designs or fills what they leave open; those entries await the user's
sign-off like any area's.

## Rules

1. **No `cargo` in any form**, and no git command. The user compiles once the stage is done, so
   each area reads its own files for type and borrow errors before handing them back.
2. **No tests and no examples.**
3. **Own files only.** An area edits the files assigned to it below and no others. A change needed
   in another area's file, a new `pub(crate)` item or a changed signature included, is written as
   a request in `divergences/race2a-<area>.md` naming the file, the item and the reason; the
   coordinator routes it to the owner. The root `Cargo.toml` belongs to the coordinator: a new
   workspace dependency is a request too.
4. **The contracts below are frozen.** An item listed under "Cross-area contracts" keeps its name,
   fields and signature. Its owner may add private helpers and fields no other area reads.
5. **Public signatures are frozen.** A public item keeps the signature the spine gives it.
   Adding, renaming or reshaping one is a divergence.
6. **Every divergence is logged** in `divergences/race2a-<area>.md`, one entry each: what the
   design says (or that it is silent), what was written, and why. A behaviour the design states
   and the code does differently is a divergence even when no signature changes.
7. **Code style.** Rust 2024, stable, MSRV 1.88. The core depends on no async runtime. Comments
   say what the code cannot: why, ordering constraints, invariants; public docs say when to use an
   item and what the design states about it. No labels, no step narration, no section dividers.
8. **`todo!()` is the only panic the spine leaves**, and every one is replaced. The core and the
   macros never panic on a path the design gives a typed error.

## Areas and files

Every source file of the race's crates belongs to exactly one area.

### CX. Core extensions

| Files | Holds |
|---|---|
| `crates/ulo/**` | every core file: the extensions are in `transport/{mod,controller,handler,inputs,metadata,enhancer,pipeline,server}.rs`, `execution/mod.rs`, `app/{mod,handle,shared}.rs`, `module/{def,handle}.rs`, `graph/{mod,wire}.rs`, `error/{mod,configure}.rs`, `__private.rs`, `lib.rs` |

Left to fill: `dispatch_late` and `recover` in `transport/pipeline.rs`; the handler's
`HandlerDecl::dependencies` as roots of steps 3 and 5 of the wiring pass (`graph/visibility.rs`,
`graph/scopes.rs`), the per-handler input check included; a module declaration of an input a
transport also declares with another seeder, and reports printing a transport origin as "declared
by transport `Http`" (`graph/wire.rs`, `error/wiring.rs`).

Design sections: transports DESIGN §2.1 (X1, X2 core side), §2.3 (X7), §2.5 (X3), §2.6 (X5),
§2.7 (X6), §2.10 (X4, X8), §3.3 (X9, and the pre-dispatch stage's use of `recover` and
`catch_panic`), §11, §13; DESIGN §3.3, §4 (`ModuleDef::with`), §6.4, §10.1 step 5, §13.

### MX. Macros and the handler protocol

| Files | Holds |
|---|---|
| `crates/ulo-macros/**` | `#[routes]` (keys, shared values, checks reads, `#[meta]`), `__enhancer_specs!` with the shared-value argument, the `meta` marker, the providers grammar (`with`, `with(scope)`, the bare closure, `K: with`) |
| `crates/ulo-handler-codegen/**` | the `__handler` grammar (`protocol`), `params`, `body`, `reply`, `keys`, `shared`, `emit` |

Left to fill: every `todo!()` in `ulo-handler-codegen`: `params::receiver_kind`,
`body::consumer_checks`, `reply::{reply_call, rewrite_opaque_returns}`,
`keys::{key_const, scoped_keys, assertions}`, `shared::{shared_values, construct, mount_generics}`,
`emit::{MountFn::emit, call_closure, call_body, dependencies, metadata}`.

Design sections: transports DESIGN §1 (the codegen row), §2.1, §2.2 (the pairwise assertion),
§2.3 (the reply probe call, `use<>`, `self: Arc<Self>`), §2.5 (`meta`), §4.1 (the X15 call a
gateway's message handler makes), §12 (compile rows); DESIGN §4 (the providers grammar), §7.

### T. Transport-neutral crates

| Files | Holds |
|---|---|
| `crates/ulo-transport/**` | `FromCall`, `Injected`, `ExtractError`, `IntoReply`, `IntoReplyError`, `ErrorKind`, `Classify`, `CallError`, `Details`, `Valid`, `Validate`, `Admission`, `span`, `Tracked`, `__private` (`Param`, `controller`, the reply probe) |
| `crates/ulo-transport-macros/**` | `#[derive(Classify)]`, `#[derive(Validate)]` |

Design sections: transports DESIGN §0.6, §2.2, §2.3, §2.4, §2.6 (`Tracked`), §2.8, §2.9, §12.

### N. Networking and runtime

| Files | Holds |
|---|---|
| `crates/ulo-net/**` | `Endpoint`, `EndpointSpec`, `ListenerName`, `Activation`, `BoundListener`, `bind_all`, `Tls` |
| `crates/ulo-tokio/**` | `Timer`, `shutdown_signal`, `spawn`, `spawn_in` |

Design sections: transports DESIGN §2.7, §8, §9 (the socket-activation protocol the child side
reads), §12 (startup rows); DESIGN §3.9.

### H. HTTP: requests, routing and replies

| Files | Holds |
|---|---|
| `crates/ulo-http/Cargo.toml`, `src/lib.rs` | the crate's manifest and re-exports |
| `crates/ulo-http/src/{transport,cx,body,request,limits}.rs` | `Http`, `RequestHead`, `ClientAddr`, `HttpCx`, `PathParams`, `HttpBody`, `Request`, `ConnInfo`, `TlsInfo`, `OnUpgrade`, `Upgraded`, `KB`, `MB`, `BodyLimit`, `Timeout` |
| `crates/ulo-http/src/router/**` | `Router`, `RouteEntry`, `RouteTarget`, `Routed`, `RouteError`, `Pattern`, `ScopePattern` |
| `crates/ulo-http/src/extract/**` | the extractors and the `Path<T>` check |
| `crates/ulo-http/src/{response,sse,render}.rs` | replies, SSE, the problem-details rendering |
| `crates/ulo-http/src/__private.rs` | `HttpHandler`, `HandlerFn`, `PathProbe` |

Design sections: transports DESIGN §2.3 (the late path), §2.4 (HTTP renderings), §2.6 (SSE's
end), §3.1, §3.2, §3.5 (the hand-off at routing), §3.6, §12.

### P. HTTP: the pipeline and the server

| Files | Holds |
|---|---|
| `crates/ulo-http/src/{middleware,pre_dispatch,cors,tower_bridge,upgrade}.rs` | `Middleware`, `middleware::Next`, `PreDispatch`, `Stage`, `ScopedStage`, `Cors`, `Service`, `ErasedLayer`, `UpgradeHandler`, `Upgrades` |
| `crates/ulo-http/src/{backend,service,server}.rs` | `Backend`, `BackendLimits`, `HttpConfig`, `AppService`, `ServiceInner`, `Server<B>` |

Design sections: transports DESIGN §2.7, §2.8, §3.3, §3.4, §3.6, §3.7, §10 (the HTTP column),
§12.

### HM. HTTP attributes and the axum backend

| Files | Holds |
|---|---|
| `crates/ulo-http-macros/**` | `#[get]` .. `#[options]`, the compile-time pattern check |
| `crates/ulo-http-axum/**` | `Axum`, `Server`, the conversion, the TLS listener |

Design sections: transports DESIGN §1 (the macro row), §2.1–§2.3 as consumed through
`ulo-handler-codegen`, §3.1, §3.7 (the axum row), §12 (an invalid route literal).

## Cross-area contracts

Each row is an item one area calls in another. The owner keeps it as the spine writes it.

### Owned by CX

| Item | Called by | For |
|---|---|---|
| `Transport::{KEY, inputs}`, `Inputs::input` | H (`Http`), MX (the key constant) | X1, X4 |
| `HandlerSpec::{new, controller, method, meta, route, shape, dependencies}`, `Mount::handler`, `Mount::once` | MX (`emit`) | X3, X15 |
| `Metadata::{new, controller, method}` | MX (`emit::metadata`) | X3 |
| `EnhancerSpec::{guard_arc, interceptor_arc, error_handler_arc}` | MX (`__enhancer_specs!`) | X2 |
| `__private::{key_in, Shared}` | MX (generated code) | X1, X2 |
| `__private::factory::{Probe, FallibleBinding, PlainBinding, ProviderScope}` | MX (`#[module]` providers) | the `with` grammar |
| `MountedHandler::{info, module, handler}`, `HandlerInfo` accessors | H (router), P (service) | routing, metadata |
| `Execution::{open, seed, route_to, cancel_with}`, `ExecutionRef::{cancel_reason, is_late, handler, report_stream_end}` | P (service), H (SSE, render), T (`Tracked`) | X5, X7, X8 |
| `dispatch`, `dispatch_late`, `LateOutcome`, `recover` | P (service), H (SSE) | the pipeline, X7, misses |
| `AppHandle::{root, redact, catch_panic, handlers}`, `DispatchStage::PreDispatch` | P (service), H (extractors' `Malformed`) | X8, X14, pre-dispatch panics |
| `Mounted::{handlers, app, timer, module_meta}`, `ModuleRef::with_execution` | P (server, stage) | X9 |
| `Server::{prepare, bind, bound}`, `BoundAddr::{new, tls}` | P (server), N (re-export) | X6 |
| `EndStream` | H (SSE) | X7 |

### Owned by MX

| Item | Called by | For |
|---|---|---|
| `ulo_handler_codegen::Paths::new` | HM | generated paths |
| `protocol::{take_handler_attr, HandlerTokens, MetaTokens}` | HM | reading `__handler` |
| `params::{analyze, HandlerSig, Param, Receiver}` | HM | the handler's signature |
| `reply::rewrite_opaque_returns` | HM | `+ use<>` |
| `emit::{MountFn, call_closure, call_ident}` | HM | the three owed items and the handler value's call |
| the generated names `__ULO_KEY_<name>`, `__ULO_CHECKS_<name>`, `__ulo_mount_<name>`, `__ulo_shared`, `__ulo_call` | `#[routes]` and every transport attribute | the protocol in `ulo-handler-codegen/src/protocol/mod.rs` |

### Owned by T

| Item | Called by | For |
|---|---|---|
| `__private::{Param, ViaCall, ViaContainer, controller}` | code MX emits | parameter extraction, the controller |
| `__private::{IntoReplyProbe, ViaCallError, ViaBoxError, ViaValue}` | code MX emits | the reply probe |
| `FromCall`, `IntoReply`, `IntoReplyError::new`, `ExtractError` variants | H (extractors, replies) | extraction and replies |
| `CallError::{from_boxed, summary, kind, message, details, challenge}`, `ErrorKind::as_str` | H (render, SSE) | rendering |
| `Validate` | H (`Json`, `Form`, `Query`, `Path` delegate) | `Valid<P>` |
| `Admission::{new, retry_after, try_admit}` | P (service) | load shedding |
| `span::call` and its field constants | P (service) | spans |
| `Tracked::new` | H (streaming replies, SSE) | stream ends |

### Owned by N

| Item | Called by | For |
|---|---|---|
| `EndpointSpec::resolve`, `Endpoint` | P (server) | endpoints in `prepare` |
| `Activation::{get, contains}` | P (server) | inherited sockets in `prepare` |
| `bind_all`, `BoundListener::{local_addr, into_std}` | P (server), HM (axum adopts) | binding |
| `Tls::load`, `TlsAcceptor` | P (server), HM (axum's listener) | TLS |

### Owned by H

| Item | Called by | For |
|---|---|---|
| `CxInner` and its fields, `MatchedRoute`, `PathParams.pairs` | P (service builds the context) | the context |
| `Router::{build, route}`, `Routed`, `RouteTarget`, `RouteError` | P (server, service) | routing |
| `router::pattern::{Pattern, ScopePattern, PatternError}` | P (stage scopes, upgrade paths) | patterns |
| `render::{status_of, problem, render_error, draining, shed, late_event}` | P (service) | rendering |
| `__private::{HttpHandler, HandlerFn, PathProbe, ViaPath, NotPath, PathCheck, Method, transport}` | code HM emits | the handler value |
| `HttpBody::new`, `Request` fields, `ConnInfo::new` and setters, `OnUpgrade::new`, `Upgraded::new` | HM (axum's conversion) | the backend edge |

### Owned by P

| Item | Called by | For |
|---|---|---|
| `Stage::scoped_for`, `ScopedStage` | H (`Router::build`) | each route's scoped entries |
| `HttpConfig` fields | H (`CxInner`, render), HM (axum reads it in `bind`) | settings |
| `Backend`, `BackendLimits::NONE` and setters, `AppService::call` | HM (axum) | the backend SPI |

## Points each area resolves in its own log

Not settled by the designs or by the spine. The owning area decides and logs the decision.

- **CX.** Whether an `on_stream_end` callback that panics is caught, since `Tracked` reports from a
  `Drop`; the module `recover(None, ..)` resolves the global handlers with.
- **MX.** The span of each generated item; the exact text of the key-assertion and body-consumer
  messages beyond the design's samples; the generic-controller form of the checks constant.
- **T.** `Valid<P>`'s `param` text; what the `validator` bridge offers beyond `violations`; the
  `email` rule's grammar; whether `CallError::from_boxed` looks inside a `Redacted` for a
  `CallError` past `LookupError::Construct`.
- **N.** How an inherited descriptor is checked to be a listening TCP socket; behaviour on
  Windows beyond `Unsupported`.
- **H.** Percent-decoding of path parameters (`+` stays literal); how a `HEAD` answered by `GET`
  drops its body; how SSE keep-alive comments interleave with a slow stream.
- **P.** Where a request carries its continuation through a tower layer (the request's extensions
  is the spine's assumption); whether `tower`'s `util` feature is needed (a request to the
  coordinator); how a route timeout is armed on the app's `Timer`.
- **HM.** One `axum::serve` per listener or one accept loop; how a failed TLS handshake is logged.
