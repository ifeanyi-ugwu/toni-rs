# Divergences: race 2b, area G (gRPC)

Every place area G departs from `transports/DESIGN.md` §6, the twentieth response, `REVIEW_2B.md`
or the spine's surface (`race2b-S.md`), and every shape it fills that they leave open. Each entry
gives what the design says, what was written, and why. All await the user's sign-off.

No cargo was run. Every claim about what compiles is read from the sources and the registry
(tonic 0.14.6, tonic-prost, tonic-health, tonic-reflection, tonic-types and tonic-prost-build
0.14.6, prost-build 0.14.4, hyper 1.11.1, hyper-util 0.1.21); the items a compile would have to
show are listed under "Not verified" at the end.

Files written: `crates/ulo-grpc/Cargo.toml`, `src/{transport, extract, dispatch, server, status,
pre_dispatch, health, reflection, client, __private}.rs` (`lib.rs` and `method.rs` as the spine
wrote them); `crates/ulo-grpc-macros/src/method.rs`; `crates/ulo-build/src/{lib, markers}.rs`;
and, under the coordinator's exception, `crates/ulo-transport/src/error.rs`. No `todo!()` remains in
the three crates.

## The one item outside G's files

### 1. `CallError::with_grpc_code` and `CallError::grpc_code`

- **Design:** decision 2, "`Conflict` maps to gRPC ABORTED ... `CallError::grpc_code(..)` overrides
  it per error"; the spine left it unowned (`race2b-S.md`, requests to G).
- **Written:** in `crates/ulo-transport/src/error.rs`, a private field `grpc_code: Option<i32>` on
  `CallError`, set to `None` by `CallError::new` and copied by `summary()`; the builder
  `pub fn with_grpc_code(self, code: i32) -> Self` and the accessor
  `pub fn grpc_code(&self) -> Option<i32>`. Nothing in `lib.rs` changed: they are methods on a type
  already exported. `ulo_grpc::to_status` uses the code when it lies in 1 to 16 and the kind's code
  otherwise.
- **Why:** the design's single name `grpc_code(..)` cannot be both the builder and the accessor
  `ulo-grpc` needs to read a private field from another crate. The pair follows `with_details` and
  `details()` beside it. The code is `google.rpc.Code`'s number, an `i32`, because `ulo-transport`
  depends on no gRPC crate.

## The dispatcher

### 2. Built on tonic's codec layer, not on `tonic::server::Grpc` and the shape-service traits

- **Design:** §6.1, one tower service "through `tonic::server::Grpc::new(ProstCodec<..>)` and its
  `unary`, `server_streaming`, `client_streaming` and `streaming` methods, each over a service type
  implementing the matching `UnaryService` .. trait". The plan leaves G to confirm it suffices.
- **Written:** the dispatcher decodes with `tonic::Streaming::new_request(ProstDecoder<T>, body,
  None, Some(4 MiB))` inside the extractors and encodes with `tonic::codec::EncodeBody::new_server
  (ProstEncoder<T>, ..)`, the two pieces `Grpc` composes, both public. `Grpc` and the four traits
  are not used.
- **Why:** `Grpc::unary` and its siblings decode the request, call the service and encode its
  answer in one call, and the service answers an unencoded `tonic::Response<M>`. The spine's
  `Grpc::Reply` is the encoded `http::Response<tonic::body::Body>` that interceptors read and
  write, which has to exist inside `ulo::dispatch`; and `Grpc` decodes before the service runs,
  where the guards would then run after a decode, against §2.2's "parameters extract inside
  dispatch's call, after every guard admits". The traits suffice for tonic's model of a handler;
  they do not fit one whose interceptors see the encoded reply. Answering a decode failure as an
  `ExtractError` through the error handlers (§12) needs the decode in an extractor too.

### 3. How `grpc-timeout` is recovered (G's open point)

- **Design:** open: how the dispatcher recovers `grpc-timeout` from a bare `http::Request`.
- **Written:** read from the request's headers before the execution opens: `TimeoutValue` of one to
  eight ASCII digits, `TimeoutUnit` in `H M S m u n`. The deadline is `Timer::now()` plus the value,
  on `ExecOptions::deadline`. The whole pipeline is raced against `Timer::sleep(value)`: when the
  sleep wins, the pipeline is dropped, the execution cancelled with `Deadline`, and the call answers
  DEADLINE_EXCEEDED, offered to no error handler. When the reply wins, the sleep moves into the
  response body, which ends with DEADLINE_EXCEEDED trailers, the execution cancelled with
  `Deadline`, if it passes while the reply streams. A value off the grammar is ignored and logged at
  `debug`; the call runs without a deadline.
- **Why:** the header is on the request as hyper hands it over. HTTP's route timeout offers its
  `Timeout` to the error handlers under a grace bound (§3.6); §6.2 says only "answers
  DEADLINE_EXCEEDED", and a caller whose deadline has passed has stopped waiting, so no grace was
  added. An error a handler still returns after the deadline renders DEADLINE_EXCEEDED, by
  `cancel_reason()`, as §2.4 requires.

### 4. `ClientCancelled` fires on any abandonment hyper reports

- **Design:** §2.6, `ClientCancelled` for "a gRPC `RST_STREAM(CANCEL)`", `Disconnected` for "an h2
  `RST_STREAM`" or a closed connection.
- **Written:** `ClientCancelled` when hyper drops the call's future before it answered, and when it
  drops the reply body before its end.
- **Why:** hyper reports neither the reset's error code nor whether the connection closed; it drops
  the future or the body in both cases. A gRPC caller abandoning a call resets with `CANCEL`, so
  that is the reason recorded.

### 5. Compressed requests answer UNIMPLEMENTED

- **Design:** silent.
- **Written:** a call whose `grpc-encoding` is present and not `identity` answers UNIMPLEMENTED with
  `grpc-accept-encoding: identity`, before dispatch and offered to no error handler.
- **Why:** the workspace enables none of tonic's compression features, and the gRPC compression
  specification prescribes this answer for an encoding the server does not support.

### 6. Health and reflection are routed after the unscoped sub-step, with no scoped entries

- **Design:** §6.2 adds both "to the route table"; silent on the stage.
- **Written:** the unscoped `PreDispatch<Grpc>` entries run for every call, health and reflection
  included; a path under `grpc.health.v1.Health` or a reflection service then goes to the tonic
  service, with no scoped entries and no error handlers, since no handler matched.
- **Why:** a scoped entry is scoped by a handler's path, and the built-in services have no handler.

### 7. Two refusals answered directly, as the drain's is

- **Design:** §6.2: "Over the server's in-flight limit, calls get UNAVAILABLE".
- **Written:** over the server's `max_inflight` or the connection's `max_per_connection`, and during
  the drain, the call answers UNAVAILABLE before an execution opens, so no pre-dispatch entry and no
  error handler runs. No `Retry-After` is sent: gRPC has none, and clients retry UNAVAILABLE.
- **Why:** the same placement HTTP's 503 shed has (§3.6, `service.rs`).

### 8. A `tonic::Status` returned as an error reaches the wire as it stands

- **Design:** silent; §2.4 maps every error by kind.
- **Written:** `render` downcasts the error the error handlers left to `tonic::Status` first and
  sends it unchanged; only other errors go through `CallError::from_boxed`.
- **Why:** a handler naming a code outside the eleven kinds returns a `Status`, and
  `from_boxed` would answer `Internal` for a type it does not recognise. `with_grpc_code` (entry 1)
  covers an error that also renders on other transports.

### 9. A controller's `.at(prefix)` does not apply to a gRPC path

- **Design:** §2.5, the prefix applies "to every route and gateway path of that controller"; silent
  on gRPC.
- **Written:** the path table uses the marker's `PATH` as it stands and ignores
  `HandlerInfo::prefix`; the server's docs say so.
- **Why:** a method's path is fixed by its proto, and a client dials it there. Refusing the prefix
  would refuse a controller mixing HTTP routes, which take it, with gRPC methods.

## Handlers and the attribute

### 10. The attribute writes a hidden method, and the reply goes through gRPC's own probe

- **Design:** §6.1, "the attribute checks the signature against the marker's types and shape through
  trait bounds"; §2.3, a handler's return converts through `IntoReply<T>` and the reply probe.
- **Written:** `#[method(Marker)]` emits, beside the handler, `#[doc(hidden)] async fn
  __ulo_grpc_<name>(..same parameters..) -> Result<ulo_grpc::__private::Answer, ulo::BoxError>`,
  which calls the handler and converts its output through `ulo_grpc::__private::ReplyProbe`, a
  six-arm autoref probe: a `Result` of a stream with an error converting into `CallError`, then one
  whose error boxes, then a bare stream, then the same three for a single message. The handler
  value, the mount function and the `ulo-handler-codegen` call read the hidden method;
  `Answer: IntoReply<Grpc>` builds the reply once the call's context is known. A stream is any
  `Stream` of `Result<M, E>` with `E: Into<CallError>`, or `Response<S>` around one; a single
  message is any prost message, `()` (prost's `google.protobuf.Empty`) included, or `Response<M>`.
- **Why:** `IntoReply<Grpc>` cannot be implemented for prost messages or streams anywhere: in
  `ulo-grpc` a blanket impl over a foreign trait puts an uncovered type parameter before the local
  `Grpc` (E0210), and the impls `ulo-build` could write in the user's crate cannot cover `()` or a
  `prost_types` message, nor any stream. `ulo-handler-codegen`'s call closure is emitted in every
  expansion and requires the handler's output to pass its probe, so the output it sees has to be a
  local type. Writing the hidden method needs no change to the codegen. Stream items require
  `E: Into<CallError>`, as SSE's `SseItem` does, so an item's kind survives into the late path.

### 11. The marker checks, by const generics at the concrete site

- **Design:** "trait bounds on the marker" (§12), mechanism unwritten.
- **Written:** in the hidden method, one `ParamProbe` call per parameter and the reply probe, each
  with the const argument `{ streams_request(<Marker as Method>::SHAPE) }` or `streams_reply(..)`.
  `Message<T>` and `Request<T>` declare `(Shaped<false>, T)`, `Streaming<T>` `(Shaped<true>, T)`, a
  stream reply `(Shaped<true>, Item)` and a message reply `(Shaped<false>, M)`; each is compared with
  `(Shaped<SHAPE streams>, Marker::Request)` or `(.., Marker::Response)`. A mismatch leaves
  `ParamCheck::matches` or `Checked::checked` uncallable, E0599 naming both types; any other
  parameter type compares `()` with `()`.
- **Why:** `Method` carries the shape as a `const SHAPE: Shape`, which a trait bound cannot read;
  the marker is concrete at the call site, where a const argument can. A handler reading no message
  at all is accepted, its request left unread.

### 12. `GrpcHandler` lost its `shape` field

- **Spine:** `GrpcHandler { path, shape, call }`.
- **Written:** `GrpcHandler { path, call }`; `new::<M, F>(call)` keeps its signature.
- **Why:** nothing reads the shape: `HandlerInfo` carries it from `HandlerSpec::shape`, and the
  dispatcher needs none, since the extractors decide how the body is read.

## Server, health, clients

### 13. `GrpcHealth::module()`, which binds `Dep<GrpcHealth>`

- **Design:** §6.2, "`fw-grpc` binds it as `Dep<GrpcHealth>`"; no module is named, and the spine
  wrote none.
- **Written:** `pub fn GrpcHealth::module() -> ulo::DynamicModule`, a global module binding one
  `GrpcHealth` and exporting it. The server reads `GrpcHealth` from the app in `prepare` and serves
  `grpc.health.v1` from its reporter; without the import it keeps a reporter of its own, and health
  still answers.
- **Why:** a binding needs a module, and a server cannot add one to a frozen graph. Health is served
  either way; the import is what makes the reporter injectable.

### 14. Health statuses

- **Design:** SERVING for every known service after `bind`, NOT_SERVING when the drain starts.
- **Written:** the known services are the services the handlers' paths name. `bind` sets each
  SERVING, overwriting what an init hook set through `Dep<GrpcHealth>`. `drain` sets the overall
  status `""` and each known service NOT_SERVING before it starts `ulo-hyper-serve`'s drain, so a
  client polling health stops sending before the GOAWAY.
- **Why:** the design's "every known service" names no source; the path table is the one the server
  has.

### 15. Reflection with no descriptor set

- **Design:** reflection from the generated descriptor set, on in debug builds.
- **Written:** with reflection on and no `file_descriptor_set(..)` given, the two reflection
  services still serve, describing only themselves. A set that does not decode is a `Configure`
  failure.

### 16. Client endpoints

- **Design:** §6.3, `GrpcClientModule::<C>::for_root(endpoint)`, the channel lazy.
- **Written:** `register` binds the parsed endpoint through `try_value`, so a bad URI is a wiring
  error, and the client through `try_singleton`, which applies the TLS configuration and calls
  `connect_lazy()` when the app connects; `C` is exported. An `https` URI without `.tls(..)` trusts
  the platform's roots. The identity is the client type with the URI and the TLS roots, labelled
  `GrpcClientModule`. `ulo-grpc` enables tonic's `tls-ring` and `tls-native-roots`.
- **Why:** loading the platform's roots reads the trust store and `connect_lazy` spawns onto the
  runtime, neither allowed in a synchronous, I/O-free `register`. tonic's own `Endpoint::new`
  applies the platform's roots to an `https` URI, and `from_shared` does not; the default matches
  what a tonic user expects.

### 17. `outgoing` reads tokio's clock

- **Written:** `outgoing(exec, request)` calls tonic's `Request::set_timeout` with the deadline less
  `tokio::time::Instant::now()`, at least one nanosecond.
- **Why:** an `ExecutionRef` reaches no `Timer`, and `ulo-tokio`'s `Timer::now` reads the same
  clock, paused test clocks included.

### 18. A request message over 4 MiB

- **Design:** silent.
- **Written:** the extractors decode with tonic's own 4 MiB limit; a larger message fails as
  `ExtractError::TooLarge`, rendered INVALID_ARGUMENT by its kind.
- **Why:** the spine's `Server` has no message-size setting; adding one is a public addition left
  for sign-off.

### 19. `grpc-status-details-bin`

- **Design:** §6.2 and R32, the details through tonic-types' `ErrorDetails`, `Detail::Json` packed by
  hand.
- **Written:** as designed. `ErrorDetails` holds one of each message, so every `FieldViolations`
  detail's violations go into the one `BadRequest` and every `Help` detail's links into the one
  `Help`; the first `ErrorInfo` and the first `RetryAfter` are kept. `Detail::Json` values are
  appended to the `google.rpc.Status` tonic-types wrote, read back with prost. An error with no
  details sends no trailer.

## `ulo-build`

### 20. Markers, the client impl and the descriptor constant

- **Design:** §6.1, R30; spine entries 31, 34, 35.
- **Written:** `compile` builds a `prost_build::Config` (vendored `protoc` and its well-known-type
  includes unless `PROTOC` is set) with a `ServiceGenerator` wrapping tonic-prost-build's, built with
  `build_server(false)`. After tonic writes a service it appends `pub mod <service_snake> { pub
  struct <Method>; impl ::ulo_grpc::Method for .. }`, the module named by tonic-build's own
  snake-casing so it sits beside `<service_snake>_client`, and, with clients on, the
  `ulo_grpc::GrpcClient` impl for `<Service>Client<Channel>`. A message path prost made relative
  gains `super::`; `()` and `::prost_types::..` stay. `finalize_package` appends
  `pub const FILE_DESCRIPTOR_SET: &[u8] = include_bytes!("<absolute path>")` when a descriptor set
  path is set, a relative path taken from the build script's directory.
- **Why:** no server trait is generated because nothing implements it. `include_bytes!` resolves a
  relative path against the generated file in `OUT_DIR`, hence the absolute path. The generated code
  names `tonic`, `tonic-prost` and `prost`, which the user's crate depends on, as with tonic's own
  build step; the crate docs say so.

## Requests for other areas

None. No workspace dependency is added: tonic's `tls-native-roots` pulls `rustls-native-certs`
0.8, present in the offline registry.

## Not verified

- Nothing was compiled. In particular: the six-arm autoref ranking of `ReplyProbe` with a const
  generic turbofish on each arm's method, and rustc accepting the anonymous const
  `{ streams_reply(<Marker as Method>::SHAPE) }` inside a generic controller's impl, where it names
  no generic parameter; coherence accepting `ReplyValue` for `T: prost::Message` beside
  `Response<T>`, and `ReplyStream` for `S: Stream` beside `Response<S>`, by negative reasoning on
  the local `Response`.
- `tonic_health`'s `HealthServer<HealthService>` and the opaque reflection servers meeting
  `builtin`'s bounds (`Clone + Send`, `Service<http::Request<HttpBody>>` with `Infallible`).
- The response body `HttpBody`, which is `!Sync`, meeting hyper's HTTP/2 executor bounds; the HTTP
  backend already serves it through hyper's auto builder.
- The end-to-end behaviour: GOAWAY on drain, `REFUSED_STREAM` past `max_concurrent_streams`, and
  hyper dropping a call's future on a client reset, which `ClientCancelled` relies on.
