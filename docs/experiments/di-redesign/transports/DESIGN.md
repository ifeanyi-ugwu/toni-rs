Below is the transport-layer design. It's written to sit beside DESIGN.md: the same numbering style, with brackets referring to this brief's capability list. Thirteen SPI extensions are named and numbered **X1–X13**, each at the point where a requirement forces it. Section 11 collects them, section 12 is the refusal table, and section 13 lists the defaults I chose where the brief left a choice open.

---

# Transport layer: design

## 0. Principles

1. **The core's pipeline is the only pipeline.** Every transport opens an `Execution`, seeds its inputs, and calls `fw::dispatch`. No transport writes its own guard walk, error-handler walk, or panic catch.
2. **Transport crates own the wire, never the semantics.** Extraction, answers, errors, metadata, and cancellation are defined once in `fw-transport`. Each transport only says how they look on its wire.
3. **Specifications decide.** HTTP follows RFC 9110/9112/9113, problem details follow RFC 9457, SSE follows the WHATWG HTML spec, WebSocket follows RFC 6455, gRPC follows its HTTP/2 protocol spec and the canonical status codes, MQTT follows v5, AMQP follows 0-9-1 (what RabbitMQ speaks), GraphQL follows GraphQL-over-HTTP and graphql-transport-ws, and span attributes follow OpenTelemetry semantic conventions. Where no specification exists (the WebSocket message envelope, the RPC frame grammar, the backend SPI), the design defines one and says so.
4. **Everything the wiring pass can see, it checks.** Global middleware, gateways, patterns, and gRPC methods are declared through module metadata or mounted handlers, so they're validated at `wire()` like any binding. Server builders in `main` carry only addresses, TLS, and limits.
5. **Transports may depend on tokio. The core still doesn't.** Every listed backend and broker client is tokio-based, so transport crates use tokio I/O traits. `fw-tokio` supplies the core's `Timer`, signals, and spawn helpers.

---

## 1. Crate layout

| Crate | Contents | Meets the core through |
|---|---|---|
| `fw-transport` | Transport-neutral pieces: `Param`, `Answer`, `CallError`, `ErrorKind`, `Details`, `ExtractError`, validation, `Metadata`, `Admission` (load shedding), span helpers, stream-end tracking | `dispatch`, `Execution`, `ErrorHandler` |
| `fw-transport-macros` | `#[derive(Validate)]`, `#[derive(Classified)]` | — |
| `fw-handler-codegen` | A plain library (not a proc-macro crate) of `syn` functions every transport's attribute macro calls: parameter analysis, the body-consumer check, answer probing, the `__handler` protocol, key and shared-value emission. A user-written transport's macro crate uses it too | the `__handler` protocol (X1, X2) |
| `fw-net` | Endpoints (address, port 0, inherited socket), bound-address reporting, TLS loading with rustls, socket-activation parsing | `Server::prepare` / `bind` (X6) |
| `fw-http` | `Http`, `HttpCx`, the router, extractors, responses, SSE, global and per-module middleware, CORS, tower bridging, WebSocket upgrade hand-off, `HttpBackend` SPI | `Transport`, `Server`, `Controller`/`Mount` |
| `fw-http-axum`, `-actix`, `-salvo`, `-poem`, `-rocket` | One `HttpBackend` each | `HttpBackend` |
| `fw-ws` | `Ws` and `WsConnect`, gateways, sessions, connection hooks, the message envelope, rooms and broadcast, `BroadcastAdapter` SPI, a standalone WebSocket server for a separate port | `Transport` ×2, `Server` |
| `fw-ws-redis` | `BroadcastAdapter` over Redis pub/sub | `BroadcastAdapter` |
| `fw-rpc` | `Rpc`, `RpcCx`, the frame grammar, pattern routing, all four call shapes, `RpcClient`, the `Link` SPI | `Transport`, `Server` |
| `fw-rpc-tcp`, `-udp`, `-nats`, `-redis`, `-amqp`, `-mqtt`, `-kafka` | One `Link` each | `Link` |
| `fw-rpc-conformance` | A shared test suite every `Link` runs | `Link` |
| `fw-grpc` | `Grpc`, `GrpcCx`, the `Method` trait, status and details mapping, reflection, health, client modules (on tonic) | `Transport`, `Server` |
| `fw-grpc-build` | `build.rs` code generation with protox (no system protoc) plus prost and tonic generators | — |
| `fw-graphql` | GraphQL-over-HTTP endpoint, graphql-transport-ws gateway, playground, `Engine` SPI | `fw-http`, `fw-ws` |
| `fw-graphql-async`, `fw-graphql-juniper` | One `Engine` each | `Engine` |
| `fw-tokio` | `Timer`, `shutdown_signal()`, spawn helpers | `Timer`, `Signal` |
| `fw-dev` (installs `cargo fw`) | Watch, rebuild, restart, socket holding | socket activation (`fw-net`) |

Each transport crate re-exports its attribute macros from one proc-macro crate (`fw-http-macros` and so on). Each of those is a thin layer over `fw-handler-codegen`.

---

## 2. Common to every transport

### 2.1 Handlers and keys [1][12][13]

A handler is a method on a controller, marked by its transport's attribute. One `#[routes]` impl may mix transports. DESIGN §7 already checks each handler's enhancers against its own transport's roles.

**X1: transport keys.** `Transport` gains `const KEY: &'static str` (`"http"`, `"ws"`, `"ws_connect"`, `"rpc"`, `"grpc"`). The `__handler` protocol gains one obligation: a transport's attribute must emit, next to `fn __fw_mount_<name>`, an associated constant `const __FW_KEY_<name>: &'static str = <Http as Transport>::KEY;`. An attribute macro on an impl item may emit several items, so this fits the protocol as it stands.

`#[routes]` expands first. It knows every scoped key written at the controller level (the `htpp` in `#[guards(htpp = AuthGuard)]`) and every handler name, so it emits one assertion per key:

```rust
const _: () = assert!(
    ::fw::__private::key_in("htpp", &[Self::__FW_KEY_get, Self::__FW_KEY_get_rpc]),
    "`htpp` is not the key of any handler's transport in this impl (handlers: get, get_rpc)"
);
```

Const evaluation runs after every attribute has expanded, so the constants exist by then. The assertion is spanned on the key token, which makes a misspelling a compile error (E0080) pointing at `htpp`. This closes D6.

**X2: impl-level shared values.** A controller-level `value = expr` becomes legal again [13]. `#[routes]` generates `impl Controller`, and its `mount` builds each impl-level value **once**, into an `Arc`. It passes a generated `__FwShared` struct to every per-handler mount function: `fn __fw_mount_<name>(m: &mut Mount<'_>, shared: &__FwShared)`. `EnhancerSpec` gains `guard_arc(Arc<V>)`, `interceptor_arc` and `error_handler_arc`. Each handler unsizes its own clone of the same `Arc` into its transport's role, so the role check still runs per handler. A rate limiter declared on the impl now limits the controller as a whole. The compile error from wave 2 (M 1) is removed.

### 2.2 Parameters and extraction [2][3]

```rust
/// A handler parameter read from the call.
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be read from a `{T}` call",
    note = "a transport implements `Param` for its own extractors; container types (`Dep`, `Ext`, ...) are accepted on every transport"
)]
pub trait Param<T: Transport>: Sized + Send + 'static {
    /// Whether this parameter consumes the body or payload. At most one may.
    const CONSUMES_BODY: bool = false;
    /// What it reads from the container, if anything, for the wiring pass.
    fn dependencies(_d: &mut Dependencies) {}
    fn extract(cx: &T::Cx) -> impl Future<Output = Result<Self, ExtractError>> + Send;
}
```

`fw-transport` implements `Param<T>` for every `T` on the core's own injection-point types (`Dep<U, Q>`, `Many`, `Ext`, `Option<_>`, `ModuleRef`, `ExecutionRef`), one concrete impl each. These delegate to `FromContainer` and declare their reads, so wiring checks handler parameters like any constructor's. A user-defined `FromContainer` type is wrapped as `Injected<S>`. The macro accepts it bare, because it probes concrete parameter types with autoref. Execution inputs are plain `Dep<RequestHead>`, and extension values are `Ext<CurrentUser>`. The context itself is a parameter, through `impl Param<Http> for HttpCx`.

**The single body consumer is checked at compile time and names both parameters.** For each pair of parameters, the macro emits:

```rust
const _: () = assert!(
    !(<Json<NewUser> as Param<Http>>::CONSUMES_BODY && <Form<Login> as Param<Http>>::CONSUMES_BODY),
    "`user` and `login` both consume the body; a handler reads the body once"
);
```

It's spanned on the second parameter. Pairs whose types are known not to consume the body are skipped, so a handler with n parameters emits far fewer than n² assertions in practice.

**Extraction failure has one shape everywhere:**

```rust
#[non_exhaustive]
pub struct ExtractError { pub param: &'static str, pub failure: ExtractFailure }

#[non_exhaustive]
pub enum ExtractFailure {
    Missing,                                  // a required header, query field or path segment
    Malformed(Redacted),                      // could not decode
    UnsupportedMediaType { expected: &'static str },
    TooLarge { limit: u64 },
    Invalid(Vec<FieldViolation>),             // validation rules failed
}
```

It reaches the error handlers as a `CallError` of kind `BadRequest`, or `Unprocessable` for `Invalid`, holding the `ExtractError` as its source. An error handler reshapes it with `err.downcast_ref::<CallError>()?.source_as::<ExtractError>()`. Each transport documents its rendering in §§3–7, and HTTP adds the exact statuses 413 and 415 from the failure.

**Validation** is declarative and opt-in through the parameter type:

```rust
#[derive(Deserialize, Validate)]
pub struct NewUser {
    #[validate(length(min = 1, max = 64))] name: String,
    #[validate(email)] email: String,
    #[validate(range(min = 13))] age: u8,
}

async fn create(&self, user: Valid<Json<NewUser>>) -> Result<Created<User>, CallError>
```

`Valid<P>` runs `Validate::validate` after `P` extracts, and turns violations into `ExtractFailure::Invalid`. A bridge for the `validator` crate sits behind a feature flag, for teams that already use it.

### 2.3 Answers [4]

A handler returns `()`, a value, a stream, or any of those inside a `Result`. The error side always reaches the error handlers:

```rust
pub trait Answer<T: Transport>: Send + 'static {
    fn into_reply(self, cx: &T::Cx) -> Result<T::Reply, BoxError>;
}
```

Transports implement `Answer` for their reply types: `Json<T>`, `Sse<S>`, `pb::User`, `impl Stream`, and so on. For a `Result<V, E>`, the generated code doesn't match on the *spelling*. It probes the *type* by autoref, at the concrete call site:

```rust
let out = Self::get(&this, a, b).await;              // whatever the return type is spelled as
(&&&::fw::__private::AnswerProbe(out)).answer::<Http>(&cx)
```

The probe has three arms:
- `Result<V, E>` where `E: Classified`: `Err` becomes `CallError::classified(e)`.
- `Result<V, E>` where `E: Into<BoxError>`: `Err` is boxed unchanged.
- Any `V: Answer<T>`: answered as the value.

The compiler resolves a type alias (`type ApiResult<T> = Result<T, ApiError>`) before method resolution, so an alias takes the same arm as the plain `Result`. That autoref ranking works on 1.88 was shown by probe P02c. In the value API, `Answer::from_result(r)` and `CallError::classified(e)` do the same thing explicitly.

**Errors in the middle of a stream (X7).** An `Err` from the outer `Result`, or from a stream before its first item, goes through `dispatch`'s error handlers like any other error. An `Err` *item* after the first item has been sent can no longer change a status. The core gains `fw::dispatch_late(handler, cx, err) -> LateOutcome`, which runs the same error handlers with `cx.exec().is_late() == true`:
- A handler returning `Err(e2)` reshapes the error, and the transport renders `e2` in its mid-stream form (§§3–6).
- A handler returning `Ok(_)` suppresses the error: the stream ends cleanly and the reply value is discarded.

### 2.4 One error model [5]

```rust
#[non_exhaustive]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ErrorKind {
    BadRequest, Unauthorized, Forbidden, NotFound, Conflict, Unprocessable,
    TooManyRequests, Timeout, Unavailable, Unimplemented, Internal,
}

/// A domain error that knows its transport-neutral kind.
pub trait Classified: std::error::Error + Send + Sync + 'static {
    fn kind(&self) -> ErrorKind;
    fn public_message(&self) -> Cow<'_, str> { self.to_string().into() }
    fn details(&self) -> Details { Details::default() }
}

/// The one concrete error every transport renders.
pub struct CallError { kind: ErrorKind, message: String, details: Details, source: Option<BoxError> }

impl CallError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self;
    pub fn classified<E: Classified>(e: E) -> Self;            // keeps `e` as the source
    pub fn with_details(self, d: Details) -> Self;
    pub fn kind(&self) -> ErrorKind;
    pub fn source_as<E: Error + 'static>(&self) -> Option<&E>; // reach the domain error
}
```

`#[derive(Classified)]` with `#[kind(not_found)]` is sugar for the trait impl.

`Details` is a list of typed entries chosen to map one-to-one onto `google.rpc` error details, so gRPC gets them natively and the other transports get stable JSON:

```rust
#[non_exhaustive]
pub enum Detail {
    FieldViolations(Vec<FieldViolation>),      // google.rpc.BadRequest
    ErrorInfo { reason: String, domain: String, metadata: BTreeMap<String, String> },
    RetryAfter(Duration),                      // google.rpc.RetryInfo; HTTP Retry-After
    Help(Vec<Link>),
    Json(serde_json::Value),                   // anything else; gRPC carries it as google.protobuf.Struct
}
```

**Classifying a `BoxError` at render time.** `fw_transport::classify(&BoxError) -> CallError` walks the error and maps the core's own errors:

| Error | Kind |
|---|---|
| `CallError` | its own |
| `ExtractError` | `BadRequest` or `Unprocessable` |
| `GuardRejected` | `Forbidden` |
| `PanicRecovered`, and anything `fw::is_panic` recognises | `Internal`, with a generic message |
| `LookupError::Construct { reason: Errored(r) }` | recurses into `r.downcast_ref::<CallError>()`, so a constructor's "tenant not found" becomes 404, as DESIGN §10.2 requires |
| `Closed`, `LookupError::Closed` | `Unavailable` |
| a passed deadline (`CancelReason::Deadline`, X5) | `Timeout` |
| anything else | `Internal`, with the message withheld |

**Canonical renderings:**

| Kind | HTTP | gRPC | WebSocket / RPC |
|---|---|---|---|
| BadRequest | 400 | INVALID_ARGUMENT | `"bad_request"` |
| Unauthorized | 401, with a `WWW-Authenticate` challenge (RFC 9110 requires one; the server's default challenge is configurable, and `CallError::unauthorized(challenge)` sets one per error) | UNAUTHENTICATED | `"unauthorized"` |
| Forbidden | 403 | PERMISSION_DENIED | `"forbidden"` |
| NotFound | 404 | NOT_FOUND | `"not_found"` |
| Conflict | 409 | ABORTED | `"conflict"` |
| Unprocessable | 422 | INVALID_ARGUMENT, with `BadRequest` field violations | `"unprocessable"` |
| TooManyRequests | 429, plus `Retry-After` from `RetryAfter` | RESOURCE_EXHAUSTED | `"too_many_requests"` |
| Timeout | 504 | DEADLINE_EXCEEDED | `"timeout"` |
| Unavailable | 503, plus `Retry-After` | UNAVAILABLE | `"unavailable"` |
| Unimplemented | 501 | UNIMPLEMENTED | `"unimplemented"` |
| Internal | 500 | INTERNAL | `"internal"` |

HTTP bodies are RFC 9457 `application/problem+json`: `type`, `title`, `status`, and `detail`, with `details` as an extension member. gRPC sends the status message plus a `google.rpc.Status` with packed details in `grpc-status-details-bin`. WebSocket and RPC use the envelope `{ "kind", "message", "details" }`.

### 2.5 Declared metadata [6]

```rust
#[routes]
#[meta(RequiresRole::any(["staff"]))]
impl AdminController {
    #[fw_http::delete("/users/{id}")]
    #[meta(RequiresRole::any(["admin"]), RateClass::Strict)]
    async fn delete(&self, id: Path<u64>) -> Result<NoContent, CallError> { /* ... */ }
}
```

Metadata values are plain `Send + Sync + 'static` values, built once at mount.

**X3: handler information on the execution.** `Mount::handler` takes a `HandlerDecl<T, H>` builder instead of positional arguments: `.controller(spec)`, `.method(spec)`, `.meta(Metadata)`, `.route(&'static str)`, `.shape(Shape)`. `dispatch` sets the mounted handler's `HandlerInfo` on the execution before the first guard runs, so enhancers read it through `cx.exec().handler()`:

```rust
impl Guard<Http> for RoleGuard {
    async fn can_activate(&self, cx: &HttpCx) -> Result<bool, BoxError> {
        let info = cx.exec().handler().expect("set by dispatch");
        let Some(required) = info.meta::<RequiresRole>() else { return Ok(true) };  // most specific wins
        Ok(required.allows(cx.ext::<CurrentUser>()?.roles()))
    }
}
```

- `info.meta::<T>()` returns the method's declaration if there is one, otherwise the controller's.
- `info.meta_all::<T>()` lists both tiers, method first.
- `info.entries()` lists every declaration, as (type name, tier).
- `AppHandle::handlers()` lists every mounted handler with its transport key, route or pattern, and metadata. That supports permission reports and generated documentation.

### 2.6 Cancellation and stream ends [7]

**X5: cancellation reasons.** The core gains `Execution::cancel_with(CancelReason)` and `ExecutionRef::cancel_reason() -> Option<CancelReason>`:

```rust
#[non_exhaustive]
pub enum CancelReason { Disconnected, ClientCancelled, Deadline, Drain }
```

Each transport fires the reason its wire tells it:
- `Disconnected`: a dropped response body, a closed connection, or an h2 `RST_STREAM`.
- `ClientCancelled`: a WebSocket or RPC `cancel` frame, or a gRPC `RST_STREAM(CANCEL)`.
- `Deadline`: a passed `grpc-timeout`, RPC `deadline` header, or configured route timeout.
- `Drain`: the core's end of the drain.

**Clean end versus cut off.** Every streaming answer is wrapped in `fw_transport::Tracked<S>`, which records one of two outcomes:
- `Completed`: the stream returned `None` and the transport finished writing.
- `CutOff(reason)`: dropped before that.

The handler or an interceptor observes the outcome through `cx.exec().stream_outcome()`, a future that resolves once. On the wire, each transport uses its protocol's own end marker:
- HTTP/1.1: the terminating zero-length chunk, versus a closed connection.
- HTTP/2: `END_STREAM`, versus `RST_STREAM`.
- gRPC: trailers with `grpc-status: 0`, versus a non-OK status or a reset.
- RPC: an `end` frame, versus an `err` frame or a cut link.
- WebSocket: a `complete` envelope, versus an `error` envelope or a close frame.

SSE has no end marker in its specification, so a client can't tell a clean end from a cut-off from the protocol alone. The server side still can. `Sse::end_event("end")` adds an opt-in final named event for clients that want the distinction. It's off by default, because the specification doesn't define one.

### 2.7 Binding, TLS and configuration order [9][10]

```rust
pub enum Endpoint {
    Addr(SocketAddr),                     // "0.0.0.0:8080"; port 0 allowed
    Inherited(ListenerName),              // a socket from LISTEN_FDS (systemd socket activation)
}
```

`Endpoint::parse("0.0.0.0:0")`, `Endpoint::inherited("http")` (matched by `LISTEN_FDNAMES`), and `Endpoint::inherited_index(0)` cover the three cases. An inherited socket is only accepted when `LISTEN_PID` equals the process ID, which follows the systemd protocol.

**X6: two-step binding.** `Server` gains `prepare(&mut self, Mounted<'_, T>) -> impl Future<Output = Result<(), BoxError>> + Send`. `listen()` changes its order to:

1. Call `prepare` on **every** server. That's route-table construction, duplicate checks, TLS loading and key/certificate matching, CORS validation, endpoint parsing, and checking that inherited sockets exist. Every failure is collected and reported together as `StartupError::Configure { transport, source: Redacted }`, a new variant (`StartupError` is `#[non_exhaustive]`). Nothing has bound yet. So a configuration error is always reported before a port conflict.
2. Call `bind` on each server in order. A server's `bind` is all-or-nothing for its own listeners: it closes any listener it opened before returning `Err`. `listen()` then closes the servers already bound, which it already does. So nothing is ever left half-bound.

`Server::bound(&self) -> Vec<BoundAddr>` reports actual addresses, and `App<Bound>::addresses()` gathers them, so port 0 reports the port the OS chose.

**TLS** uses rustls via `fw_net::Tls::from_pem_files(cert, key)` or `Tls::from_pem(..)`. Both are parsed in `prepare`, so a bad certificate, a key that doesn't match, or an unreadable file fails startup as `Configure`, never inside the serve loop. ALPN is set per transport (`h2` and `http/1.1` for HTTP, `h2` for gRPC). Broker links take their client libraries' TLS configuration. UDP has no `tls` method at all, because DTLS isn't supported, so trying it is E0599.

### 2.8 Load shedding [11]

`fw_transport::Admission` is a runtime-free semaphore built on `async-lock`, with two counters: one per server and one per connection where a connection exists. Over the limit, each transport refuses in its protocol's own way. §§3–6 list the refusals.

### 2.9 Spans [14]

Each transport wraps `dispatch` in a `tracing` span, created by `fw_transport::span::call(..)`. Span names and attributes follow the OpenTelemetry semantic conventions:
- HTTP: the span is named `GET /users/{id}`, with `http.request.method`, `http.route` and `url.path`.
- RPC: `rpc.system` (`"fw"`), `rpc.method` (the pattern) and `messaging.system` for brokers.
- gRPC: `rpc.system = "grpc"`, `rpc.service`, `rpc.method` and `rpc.grpc.status_code`.

Every span also carries `fw.transport` (the key) and `fw.handler`.

### 2.10 Execution inputs without user imports

**X4: transports declare their own inputs.** `Transport` gains `fn inputs(d: &mut InputDecls) {}`. The freeze calls it once per transport type, the first time a handler of that transport mounts. It records the inputs with `seeded_by` set to that transport, so users never import a module just to declare inputs:
- HTTP: `RequestHead`, `ClientAddr`.
- WebSocket: `ConnectionInfo`, `UpgradeHead`.
- RPC: `CallHeaders`, `LinkInfo`.
- gRPC: `GrpcMetadata`, `PeerAddr`.

The per-handler input check from DESIGN §6.4 then works across transports unchanged.

**X8: routing to a module after the execution opens.** HTTP's pre-routing middleware runs before the controller, and so before its module, is known. The core gains `Execution::route_to(&ModuleRef)`. It's callable once, by the holder only, before `dispatch`. The execution opens with root visibility, and after routing it switches to the controller's module. Extensions written by pre-routing middleware survive the switch.

---

## 3. HTTP

### 3.1 API

```rust
#[routes]
#[guards(http = AuthGuard)]
#[interceptors(value = Timing::new())]                     // impl-level value: built once, shared (X2)
#[meta(RequiresRole::any(["staff"]))]
impl UsersController {
    #[fw_http::get("/users/{id}")]
    async fn get(&self, Path(id): Path<u64>, user: Ext<CurrentUser>) -> Result<Json<User>, UserError> {
        Ok(Json(self.users.find(id, &user).await?))
    }

    #[fw_http::post("/users")]
    #[guards(value = RateLimit::per_second(20))]
    async fn create(&self, body: Valid<Json<NewUser>>) -> Result<Created<Json<User>>, UserError> {
        let user = self.users.create(body.into_inner().0).await?;
        Ok(Created::at(format!("/users/{}", user.id), Json(user)))
    }

    #[fw_http::get("/users/events")]
    async fn events(&self, last: LastEventId, cx: HttpCx) -> Sse<impl Stream<Item = Result<Event, UserError>>> {
        let resume = last.0;
        let exec = cx.exec().clone();
        Sse::new(self.feed.since(resume).take_until(exec.draining()))   // ends cleanly when the drain starts
            .keep_alive(Duration::from_secs(15))                          // ": keepalive" comments
    }
}

#[derive(Debug, thiserror::Error, Classified)]
pub enum UserError {
    #[error("user not found")] #[kind(not_found)] NotFound,
    #[error("email already registered")] #[kind(conflict)] Taken,
}
```

**`HttpCx`** is `Clone + Send + Sync`: an `ExecutionRef`, an `Arc<RequestHead>`, the matched route, path parameters, the client address, a take-once body slot, and an interior-mutable response-head writer. Its methods include `head()`, `method()`, `uri()`, `headers()`, `route()`, `exec()`, `ext::<T>()`, `take_body()`, `response_headers()` and `client_addr()`.

**Extractors** [17] are `Path<T>`, `Query<T>`, `Json<T>`, `Form<T>`, `Bytes`, `BodyStream`, `Multipart`, `Header<H>` (typed, from the `headers` crate), `HeaderMap`, `LastEventId`, `HttpCx`, and every container type. `Json`, `Form`, `Bytes`, `BodyStream` and `Multipart` consume the body. The body limit is a server setting (`.body_limit(2 * MB)`) that `#[meta(BodyLimit(..))]` can override per route. Bodies over the limit fail with 413 before deserialization.

**Responses** [18] are any `T: Answer<Http>`:
- `Json<T>`, `Bytes`, `String`, `()` (204), `Created<T>`, `NoContent`, and `(StatusCode, T)`.
- `WithHeaders<T>`, and `Response::builder()` for any status, headers and body.
- `Body::stream(s)` for a streaming body.
- `Sse<S>` with `Event::default().data(..).id(..).event(..).retry(..)`. Multi-line data is split into several `data:` lines, and keep-alive comments are written as `: keepalive`, both per the SSE spec. The content type is `text/event-stream` in UTF-8.

### 3.2 Routing [15]

`fw-http` owns the router, so every backend routes identically:
- Patterns use `{name}` segments plus an optional trailing `{*rest}`.
- A trailing slash is insignificant: both the pattern and the request path drop it, except for `/`.
- Routes are built in `prepare`. Two routes with the same method and the same normalized pattern, or two patterns that differ only in parameter names at one position (`/u/{id}` and `/u/{name}`), are a `Configure` error naming both handlers.
- `prepare` also checks every `Path<T>` against its route. It does this by running `T`'s `Deserialize` impl against a deserializer that records which field names get requested, then comparing them with the pattern's parameter names, so a mismatch is caught at startup rather than by the first request.
- A path that matches nothing answers 404. A matching path with the wrong method answers 405 with an `Allow` header, which RFC 9110 requires.
- `HEAD` is answered from the `GET` handler with the body omitted, unless a `HEAD` handler exists (RFC 9110 requires general-purpose servers to support `HEAD`).
- `OPTIONS` without a handler answers 204 with `Allow`. CORS preflight never reaches routing (§3.4).

### 3.3 Middleware [19]

Middleware is HTTP-specific, so it isn't a core role. Middleware types are still ordinary container bindings, and the metadata that declares them reports its dependencies (`Meta::dependencies`), so wiring checks them.

```rust
pub trait Middleware: Send + Sync + 'static {
    fn handle(&self, req: Request, next: MiddlewareNext<'_>) -> impl Future<Output = Response> + Send;
}
```

Middleware comes in two stages:
- **Global, before routing.** These run for every request, misses included. They can rewrite the path with `req.set_path(..)` and can answer without calling `next`, which is how CORS preflight and authentication work. They're declared in module metadata, in collection order:

  ```rust
  m.meta::<fw_http::Pipeline>()
      .global_value(Cors::new().allow_origin("https://app.example").allow_credentials(true))
      .global::<RequestId>()
      .layer(tower_http::compression::CompressionLayer::new());
  ```

- **Per-module, after routing.** These are selected by route *pattern* with exclusions, as DESIGN §7 already describes: `m.meta::<fw_http::Middleware>().apply::<AuditLog>().for_routes(["/admin/*"]).exclude(["/admin/health"])`. A module's selection applies to every matching route in the app, NestJS-style, and modules apply in collection order (§13, decision 1).

The full order per request: global middleware, routing, `route_to` (X8), per-module middleware, then `dispatch` (guards, interceptors, handler, error handlers).

### 3.4 CORS and tower [20]

`fw_http::Cors` follows the Fetch specification's CORS protocol. It answers preflights (`OPTIONS` with `Access-Control-Request-Method`) with 204 and the `Access-Control-Allow-*` headers, adds `Vary: Origin`, and never echoes `*` together with credentials. That last combination is forbidden by the spec, so it's refused in `prepare`.

`.layer(L)` accepts any `tower::Layer` over `fw_http::Service`, which is `Service<http::Request<HttpBody>, Response = http::Response<HttpBody>>`. Layers run inside the global stage, so they behave the same on every backend, including actix and rocket, which aren't tower-based.

### 3.5 WebSocket upgrades [21]

When a gateway's path matches a request carrying `Upgrade: websocket`, the router hands the request to `fw-ws` through the backend's upgrade future (§3.7). `fw-ws` completes the RFC 6455 handshake and runs the connection on the upgraded I/O. For a separate port, `fw_ws::Server::new(endpoint)` runs its own minimal HTTP/1.1 upgrade server. Gateways are identical on either.

### 3.6 Wire behavior, limits, shutdown

- **Success:** the answer's status, headers and body. Streaming bodies use chunked encoding on HTTP/1.1 and data frames on HTTP/2.
- **Failure:** RFC 9457 problem details (§2.4). Extraction failures are 400, 413, 415 or 422 depending on the `ExtractFailure`, with field violations in `details`.
- **Load shedding:** over the server's in-flight limit, a request gets 503 with `Retry-After`. On HTTP/2, `SETTINGS_MAX_CONCURRENT_STREAMS` bounds each connection, and excess streams are refused with `RST_STREAM(REFUSED_STREAM)` per RFC 9113.
- **Cancellation:** a body dropped before its end fires `Disconnected`. A route timeout (`#[meta(Timeout(..))]`) fires `Deadline` and renders 504.
- **Shutdown:** at `drain` the backend stops accepting, sends HTTP/2 GOAWAY, closes idle HTTP/1.1 keep-alive connections, and marks busy ones `Connection: close` on their next response. SSE handlers observe `draining()` and end their streams. `close` closes everything that's left.

### 3.7 Backend SPI [16], written without macros

```rust
pub trait HttpBackend: Send + Sync + 'static {
    const NAME: &'static str;
    fn limits() -> BackendLimits;                    // documented limits, checked in `prepare`

    fn bind(&mut self, listeners: Vec<BoundListener>, tls: Option<TlsAcceptor>, svc: AppService, cfg: &HttpConfig)
        -> impl Future<Output = Result<(), BoxError>> + Send;
    fn serve(&self) -> impl Future<Output = Result<(), BoxError>> + Send;
    fn drain(&self) -> impl Future<Output = ()> + Send;   // GOAWAY, close idle keep-alives
    fn close(&self) -> impl Future<Output = Result<(), BoxError>> + Send;
}

/// What a backend calls per request: global middleware, routing, dispatch, rendering.
#[derive(Clone)]
pub struct AppService { /* Arc inside */ }
impl AppService {
    pub fn call(&self, req: Request) -> impl Future<Output = Response> + Send;
}

pub struct Request {
    pub head: http::request::Parts,
    pub body: HttpBody,                              // stream of Bytes; dropping it early is a disconnect
    pub conn: ConnInfo,                              // peer address, TLS info, protocol version
    pub upgrade: Option<OnUpgrade>,                  // resolves to an AsyncRead + AsyncWrite + Send + Unpin stream
}
```

`fw_http::Server<B: HttpBackend>` implements the core `Server`. The `http` crate's types are the common language, and actix and rocket convert at their edge. A backend crate is roughly 300 lines of conversion plus the drain mapping.

`BackendLimits` is enforced in `prepare`: asking a backend for something it declares it can't do is a `Configure` error naming the limit. The table below is the starting point, and the conformance tests confirm each entry before release:

| Backend | Documented limits |
|---|---|
| axum (hyper) | none expected |
| salvo, poem (hyper) | none expected |
| actix-web | HTTP/2 only over TLS through ALPN, so h2c is refused; its multi-runtime model means our `Send` futures run on actix workers |
| rocket | inherited sockets and port 0 depend on the version's custom-listener support, and are refused where missing; graceful-shutdown timing follows rocket's own shutdown configuration |

---

## 4. WebSocket

### 4.1 API [22]–[26]

```rust
#[derive(Default)]
pub struct ChatSession { nick: Mutex<Option<String>> }

#[routes]
#[fw_ws::gateway(path = "/chat", namespace = "lobby", event = "event",
                 session = ChatSession, connect_guards(TokenGuard))]
impl ChatGateway {
    #[fw_ws::after_init]
    async fn ready(&self, gw: GatewayRef) { tracing::info!(path = gw.path(), "chat ready"); }

    #[fw_ws::on_connect]                                       // after the connect guards; may refuse
    async fn connected(&self, conn: Connection, user: Ext<CurrentUser>) -> Result<(), Refusal> {
        conn.join("lobby").await;
        Ok(())
    }

    #[fw_ws::on_disconnect]
    async fn gone(&self, conn: Connection, why: DisconnectReason) { self.presence.leave(conn.id()).await; }

    #[fw_ws::message("chat.send")]
    #[guards(value = RateLimit::per_second(5))]
    async fn send(&self, msg: Payload<ChatMessage>, session: Session<ChatSession>, rooms: Dep<Rooms>)
        -> Result<Ack, ChatError>
    {
        rooms.to_room("lobby").except([session.conn_id()]).emit("chat.message", &msg.0).await?;
        Ok(Ack::default())
    }

    #[fw_ws::message("chat.history")]
    async fn history(&self, q: Payload<HistoryQuery>) -> impl Stream<Item = Result<ChatMessage, ChatError>> {
        self.store.page(q.0)
    }
}
```

`#[fw_ws::gateway]` sits on the impl next to `#[routes]`. `#[routes]` passes it through untouched, and it then emits `impl fw_ws::GatewayConfig`, so it needs no protocol change.

**Two transports.** `Ws` covers message handlers (one execution per message). `WsConnect` covers the connection phase, with key `"ws_connect"`: its `Cx` is `ConnectCx` and its reply is admit or refuse. Connect guards are ordinary `Guard<WsConnect>` implementations, running through `dispatch` like any guard. So refusals, error handlers, kinds and panics all work the same at connect time. A connection isn't an execution. The connect phase, each message, and each connection hook are.

**Sessions** [24]: when the upgrade completes, the session is created before the connect guards run, from `Default` or from a factory declared with `session_with = |head: Dep<UpgradeHead>| ..`. `Session<T>` is a `FromContainer` type reading it, and it's dropped with the connection. Per-message state lives in the message's execution.

**Connection hooks** [25]:
- `on_connect` runs after the connect guards and can refuse.
- `on_disconnect` receives a `DisconnectReason`: `ClientClose { code, reason }`, `ServerClose { code }`, `ProtocolError`, `Drain`, or `Lost`. During the drain it runs as a terminal execution (DESIGN §6.3), so it's best effort at shutdown.
- `after_init` runs once per gateway after `listen()`, with a `GatewayRef`.

### 4.2 Wire format

The message envelope is defined here, since no specification covers one. Text frames carry JSON. Binary frames carry MessagePack, configured per gateway with `codec = msgpack`. The event field name is configurable, and graphql-ws uses `type`.

```
→ {"event":"chat.send","id":7,"data":{...}}
← {"id":7,"data":{...}}                                  single answer (ack)
← {"id":7,"data":{...}} ... {"id":7,"complete":true}     streamed answer
← {"id":7,"error":{"kind":"forbidden","message":"...","details":[...]}}
→ {"event":"cancel","id":7}                              fires ClientCancelled
```

A message without an `id` is fire-and-forget: there's no ack, and errors are logged. Control frames (ping, pong, close) are answered by the protocol layer and never reach handlers [26]. Every failure is the one `error` envelope: extraction, a guard, a handler, a panic, or an unknown event (kind `unimplemented`).

**Close codes** [23], from RFC 6455's registry:
- A connect refusal by kind: `Unauthorized` and `Forbidden` close with 1008 (policy), `TooManyRequests` with 1013 (try again later), `Internal` with 1011, and `Unavailable` with 1013.
- A gateway may map kinds to its subprotocol's own codes, as graphql-transport-ws does with 4401, 4403 and 4429. `Refusal::code(4403, "forbidden")` sets one explicitly.
- Frames over the size limit close with 1009. Binary frames on a text-only gateway close with 1003. A protocol violation closes with 1002.

**Load shedding:** over a gateway's connection limit, the connection is accepted and closed with 1013. Over the per-connection in-flight message limit, the gateway stops reading from that connection, which is backpressure rather than a refusal. Each connection's outbound queue is bounded. When it overflows, the connection closes with 1008 and the reason "slow consumer", or drops the oldest message if the gateway sets `overflow = drop_oldest`.

**Shutdown:** at the start of the drain, idle connections close with 1001 at once. Busy connections stop reading, finish their in-flight messages, then close with 1001 (DESIGN §9.5).

### 4.3 Rooms and broadcast [27]

`Rooms` is injectable. `Dep<Rooms>` addresses all gateways, and `Dep<Rooms, ChatGateway>` addresses one.

```rust
rooms.to_all().emit("notice", &n).await?;
rooms.to_room("lobby").except([me]).emit("chat.message", &m).await?;
rooms.to_client(id).emit("dm", &dm).await?;
conn.join("lobby").await;  conn.leave("lobby").await;
```

The broadcast adapter SPI, written without macros:

```rust
pub trait BroadcastAdapter: Send + Sync + 'static {
    fn publish(&self, target: Target, frame: Bytes) -> BoxFuture<'static, Result<(), BoxError>>;
    fn subscribe(&self, node: NodeId) -> BoxStream<'static, (Target, Bytes)>;
}
```

The in-memory adapter is the default. `fw-ws-redis` publishes on one channel per room or client. Each process delivers to its own local members, and room membership stays local to each process. Delivery is best effort and ordered per sender within one process. The API is identical in both modes, and multi-process mode is selected by importing `WsRedisModule::for_root(url)`.

---

## 5. RPC

### 5.1 API [28][29]

```rust
#[routes]
#[guards(rpc = ServiceTokenGuard)]
impl InvoicesController {
    #[fw_rpc::message("invoices.create")]                                  // request-reply
    async fn create(&self, req: Payload<NewInvoice>, h: CallHeaders) -> Result<Invoice, BillingError> { /* ... */ }

    #[fw_rpc::event("user.created")]                                       // fire-and-forget
    async fn on_user(&self, evt: Payload<UserCreated>) -> Result<(), BillingError> { /* ... */ }

    #[fw_rpc::message("invoices.watch")]                                   // streamed reply
    async fn watch(&self, q: Payload<Watch>) -> impl Stream<Item = Result<Invoice, BillingError>> { /* ... */ }

    #[fw_rpc::message("invoices.import")]                                  // streamed request
    async fn import(&self, rows: Inbound<InvoiceRow>) -> Result<ImportSummary, BillingError> { /* ... */ }

    #[fw_rpc::message("invoices.sync")]                                    // both streaming
    async fn sync(&self, ops: Inbound<SyncOp>) -> impl Stream<Item = Result<SyncAck, BillingError>> { /* ... */ }
}
```

The call shape comes from the signature: an `Inbound<T>` parameter means a streamed request, and a stream return means a streamed reply. The shape is recorded in the `HandlerDecl` (X3).

### 5.2 One wire grammar [30][32][33]

Frames, defined here since no specification governs them. They're JSON or CBOR depending on the link's codec:

```
req     {t,id,p,h,d}        one request, unary or server-streaming
evt     {t,p,h,d}           event, no reply
res     {t,id,d}            single reply
err     {t,id,e:{kind,message,details}}
item    {t,id,d}            reply-stream item
end     {t,id}              reply stream ended cleanly
open    {t,id,p,h}          start a streamed request
in      {t,id,d}            request-stream item
in_end  {t,id}              request stream ended
cancel  {t,id}              caller gave up: fires ClientCancelled
credit  {t,id,n}            flow control, on links without native backpressure
goaway  {t}                 server draining: no new calls on this connection
```

Reserved headers: `deadline-ms` (remaining time, which becomes the execution deadline) and `traceparent` (W3C Trace Context).

**A pattern nothing handles** answers `err` with kind `unimplemented` and the message "no handler for pattern `x`", on every link. An unhandled *event* is logged and counted, and on brokers it's acknowledged as rejected, so it can't loop on redelivery (AMQP: `basic.reject` without requeue, which routes to a dead-letter exchange if one is configured). A shape mismatch, such as `req` sent to a streamed-request pattern, answers `bad_request`.

**What a caller sees is uniform.** `RpcError` exposes a kind from §2.4 and maps link-level failures the same way everywhere: no responders, a lost link, or a broker refusal map to `Unavailable`, a client timeout maps to `Timeout`, and an oversized payload maps to `BadRequest` with `ErrorInfo { reason: "payload_too_large" }`. `fw-rpc-conformance` runs one scenario list against every link (unary, each streaming shape, cancel mid-stream, unknown pattern, deadline, binary payload, oversized payload, drain) and asserts the same caller-visible result.

**Binary payloads.** `Bytes` and `Payload<Bytes>` travel raw inside CBOR envelopes. A link configured with a JSON codec, chosen for interoperability, declares `binary: false`. A handler or client call using a binary payload on such a link is refused in `prepare` with a `Configure` error naming the pattern and the link.

### 5.3 Links [30], written without macros

```rust
pub trait Link: Send + Sync + 'static {
    const NAME: &'static str;
    fn capabilities(&self) -> Capabilities;      // binary, max_frame, ordering, shapes, native backpressure

    /// Server side: subscribe to the given patterns. Called from `Server::bind`.
    fn listen(&self, patterns: &[Pattern]) -> impl Future<Output = Result<Inbound, BoxError>> + Send;

    /// Client side: connect. `RpcClient` calls it lazily, on first use.
    fn connect(&self) -> impl Future<Output = Result<Outbound, BoxError>> + Send;
}

pub struct Delivery { pub frame: Frame, pub reply: ReplyPath, pub ack: Ack }
```

`fw_rpc::Server<L: Link>` implements the core `Server`. In `prepare` it checks each handler's shape against `capabilities()`, so a streamed shape on UDP is a `Configure` error, and a frame bigger than `max_frame` is refused at runtime with the error above.

Per-link mapping and documented limits:

| Link | Request-reply mapping | Limits |
|---|---|---|
| TCP | multiplexed by `id` over one connection, with length-prefixed frames | ordered per connection; all shapes |
| UDP | one frame per datagram, replies sent to the sender's address | unordered, no delivery guarantee, payload at most 65,507 bytes minus the envelope; **unary and events only**, streamed shapes refused at startup; no TLS |
| NATS | the reply subject (`_INBOX`); stream items on the inbox | at-most-once; ordered per publisher and subject; maximum payload read from the server's `INFO`; no-responders status maps to `Unavailable` |
| Redis | pub/sub on `pattern`; replies on a per-client reply channel | at-most-once; ordered per channel |
| RabbitMQ (AMQP 0-9-1) | `reply_to` plus `correlation_id` properties | at-least-once with ack after the handler completes, so **handlers must be idempotent**; ordered per queue with a single consumer; prefetch (`basic.qos`) is the per-connection bound |
| MQTT v5 | the v5 Response Topic and Correlation Data properties | QoS configurable; ordered per topic and QoS; maximum packet size from CONNACK |
| Kafka | a reply topic plus a correlation header | ordered per partition (the key is the correlation ID); high latency for request-reply; size limit from broker configuration |

Links connect lazily: the server side at `bind`, the client side on its first call. Load shedding refuses with `err` of kind `unavailable` over the server's in-flight limit. Where the broker offers flow control, it's used instead: AMQP prefetch, pausing Kafka partitions, and `credit` frames for NATS and Redis.

**Shutdown:** TCP sends `goaway` on every connection and finishes the calls in flight. NATS uses its drain protocol. AMQP cancels its consumers (`basic.cancel`). MQTT unsubscribes. Kafka pauses and commits offsets. In-flight calls then drain under the core's rules.

### 5.4 Client [31]

```rust
#[module(imports = [RpcClientModule::for_root(fw_rpc_nats::Nats::url("nats://bus:4222")).keyed::<Billing>()])]
pub struct OrdersModule;

#[injectable]
pub struct Checkout { billing: Dep<RpcClient, Billing> }

impl Checkout {
    async fn finish(&self, order: &Order) -> Result<Invoice, RpcError> {
        self.billing.request::<_, Invoice>("invoices.create", &NewInvoice::from(order))
            .header("tenant", order.tenant())
            .timeout(Duration::from_secs(2))
            .await
    }
}
```

The client also offers `emit(pattern, &data)`, `stream(pattern, &req)` (returns a stream), `send_stream` and `duplex`. A timeout uses the app's `Timer`. Dropping a stream or a pending request sends `cancel`. A call made inside an execution forwards the remaining deadline as `deadline-ms`.

---

## 6. gRPC

### 6.1 Generation and API [34][36]

```rust
// build.rs
fn main() -> Result<(), Box<dyn std::error::Error>> {
    fw_grpc_build::configure().compile(&["proto/users.proto"], &["proto"])?;   // protox: no system protoc
    Ok(())
}

// src/pb.rs
fw_grpc::include_proto!("users.v1");
```

For every RPC method, generation emits the messages (prost), a client (tonic), a file descriptor set (for reflection), and a marker type:

```rust
pub struct GetUser;
impl fw_grpc::Method for GetUser {
    const PATH: &'static str = "/users.v1.UserService/GetUser";
    const SHAPE: Shape = Shape::Unary;
    type Request = GetUserRequest;
    type Response = User;
}
```

Handlers bind to a marker, and the attribute checks the signature against the marker's types and shape through trait bounds, so a mismatch fails to compile:

```rust
#[routes]
#[guards(grpc = MtlsGuard)]
impl UsersGrpc {
    #[fw_grpc::method(pb::user_service::GetUser)]
    async fn get_user(&self, req: Message<pb::GetUserRequest>) -> Result<Response<pb::User>, UserError> {
        let user = self.users.find(req.0.id).await?;
        Ok(Response::new(user.into()).metadata("x-cache", "miss"))      // reply metadata [35]
    }

    #[fw_grpc::method(pb::user_service::WatchUsers)]
    async fn watch(&self, req: Request<pb::WatchRequest>, cx: GrpcCx)  // the whole request, and the context
        -> Result<impl Stream<Item = Result<pb::UserEvent, UserError>>, UserError> { /* ... */ }

    #[fw_grpc::method(pb::user_service::ImportUsers)]
    async fn import(&self, rows: Streaming<pb::UserRow>) -> Result<pb::ImportSummary, UserError> { /* ... */ }
}
```

A service method with no handler answers UNIMPLEMENTED, as the gRPC spec requires. Two handlers for one `PATH` are a `Configure` error.

### 6.2 Wire behavior [35]

- **Deadlines:** `grpc-timeout` is parsed with the spec's units (`H`, `M`, `S`, `m`, `u`, `n`) into the execution deadline. When it passes, the execution is cancelled with `Deadline` and the call answers DEADLINE_EXCEEDED.
- **Errors:** kinds map to canonical codes per §2.4. `Details` are packed into a `google.rpc.Status` and sent in `grpc-status-details-bin`.
- **Cancellation:** a client `RST_STREAM(CANCEL)` fires `ClientCancelled`.
- **Load shedding:** `SETTINGS_MAX_CONCURRENT_STREAMS` bounds each connection, and excess streams get `REFUSED_STREAM`. Over the server's in-flight limit, calls get UNAVAILABLE, which clients treat as retryable.
- **Health:** `grpc.health.v1` reports SERVING after `listen()` and switches every service to NOT_SERVING when the drain starts. It stays SERVING through the before-shutdown stage, consistent with `is_draining()`. `Dep<GrpcHealth>` sets status per service.
- **Reflection:** `grpc.reflection.v1` and `v1alpha`, from the generated descriptor set. Both are on by default in debug builds and opt-in in release builds.
- **Shutdown:** GOAWAY through hyper's graceful shutdown, then the core's drain and close.

### 6.3 Clients [36]

```rust
imports = [GrpcClientModule::<pb::UserServiceClient<Channel>>::for_root(
    GrpcEndpoint::new("https://users:443").tls(ClientTls::system_roots()),
).keyed::<UsersApi>()]

#[injectable]
pub struct Profile { users: Dep<pb::UserServiceClient<Channel>, UsersApi> }
```

The channel connects lazily (tonic's `connect_lazy`), so `connect` does no network I/O. Inside an execution, the client forwards the remaining deadline as `grpc-timeout`.

---

## 7. GraphQL [37]

```rust
#[module(
    imports   = [GraphqlModule::for_root(GraphqlConfig::at("/graphql").subscriptions("/graphql/ws"))],
    providers = [with = |q: Dep<QueryRoot>, s: Dep<SubscriptionRoot>| AsyncGraphql::new(Schema::new(q, EmptyMutation, s))],
)]
pub struct ApiModule;

#[injectable(execution)]                                    // built per execution [37]
pub struct GqlContext { user: Option<Ext<CurrentUser>>, loaders: Dep<Loaders> }
```

The engine SPI, written without macros:

```rust
pub trait Engine: Send + Sync + 'static {
    fn execute(&self, req: GqlRequest, exec: ExecutionRef) -> BoxFuture<'static, GqlResponse>;
    fn subscribe(&self, req: GqlRequest, exec: ExecutionRef) -> BoxStream<'static, GqlResponse>;
    fn sdl(


<!-- Received cut off here: the paste exceeded the chat limit partway through §7. The rest of §7 and §§8-13 (the X1-X13 summary, the refusal table, the chosen defaults) are still to come. -->
