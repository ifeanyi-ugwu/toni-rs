The transport-layer design, written to sit beside DESIGN.md: the same numbering style, with brackets referring to the capability list in `CAPABILITIES.md`. Fourteen SPI extensions are named and numbered **X1–X9** and **X11–X15**, each at the point where a requirement forces it; X10 was folded into X5 and its number is not reused. Section 11 collects them, section 12 is the refusal table, and section 13 lists the defaults taken where the brief left a choice open.

---

# Transport layer: design

## 0. Principles

1. **The core's pipeline is the only pipeline.** Every transport opens an `Execution`, seeds its inputs, and calls `fw::dispatch`. No transport writes its own guard walk, error-handler walk, or panic catch.
2. **Transport crates own the wire, never the semantics.** Extraction, answers, errors, metadata, and cancellation are defined once in `fw-transport`. Each transport only says how they look on its wire.
3. **Specifications decide.** HTTP follows RFC 9110/9112/9113, problem details follow RFC 9457, SSE follows the WHATWG HTML spec, WebSocket follows RFC 6455, gRPC follows its HTTP/2 protocol spec and the canonical status codes, MQTT follows v5, AMQP follows 0-9-1 (what RabbitMQ speaks), GraphQL follows GraphQL-over-HTTP and graphql-transport-ws, and span attributes follow OpenTelemetry semantic conventions. Where no specification exists (the WebSocket message envelope, the RPC frame grammar, the backend SPI), the design defines one and says so.
4. **Everything the wiring pass can see, it checks.** Global middleware, gateways, patterns, and gRPC methods are declared through module metadata or mounted handlers, so they're validated at `wire()` like any binding. Server builders in `main` carry only addresses, TLS, and limits.
5. **Transports may depend on tokio. The core still doesn't.** Every listed backend and broker client is tokio-based, so transport crates use tokio I/O traits. `fw-tokio` supplies the core's `Timer`, signals, and spawn helpers.
6. **Names follow std's rules.** A trait is named after its predominant method, as `Clone::clone` and `FromStr::from_str` are: `FromCall::from_call`, `IntoReply::into_reply`, `Classify::classify`, `Validate::validate`. A trait with no single method is named for its concept or role (`Iterator`, `Error`): `Transport`, `Link`, `Engine`, `Backend`, `Guard`. An error is named after its operation as users understand it, so `FromStr` fails with `ParseIntError` and `into_string` with `IntoStringError`: `ExtractError` beside `from_call`, `IntoReplyError` beside `into_reply`. An error shared by a family of operations is named after the domain (`io::Error`): `CallError`, `RpcError`, the core's `LookupError`. An error that reports a condition rather than an operation's failure is named for the condition, the class std has in `PoisonError`, written here in the past participle the core uses: `GuardRejected`, `PanicRecovered`, `TimerMissing`, `ConnectRefused`. `Closed` stays, its subject implied wherever it appears; `Redacted` is a wrapper named for what was done to its contents, not an error. `From*` builds `Self` from a source and `Into*` consumes `self`; `Try*` appears only beside an infallible sibling, so a fallible conversion with none returns `Result` under the plain name (`Endpoint::parse`, `Tls::from_pem_files`). Abbreviations are not names: `HandlerSpec`, `Inputs`.

---

## 1. Crate layout

| Crate                                                               | Contents                                                                                                                                                                                                                                                                                                                 | Meets the core through                      |
| ------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------- |
| `fw-transport`                                                      | Transport-neutral pieces: `FromCall`, `IntoReply`, `Classify`, `CallError`, `ErrorKind`, `Details`, `ExtractError`, validation, `Metadata`, `Admission` (load shedding), span helpers, stream-end tracking                                                                                                               | `dispatch`, `Execution`, `ErrorHandler`     |
| `fw-transport-macros`                                               | `#[derive(Validate)]`, `#[derive(Classify)]`                                                                                                                                                                                                                                                                             | —                                           |
| `fw-handler-codegen`                                                | A plain library (not a proc-macro crate) of `syn` functions every transport's attribute macro calls: parameter analysis, the body-consumer check, the reply probe, the `use<>` rewrite on streaming returns, the `__handler` protocol, key and shared-value emission. A user-written transport's macro crate uses it too | the `__handler` protocol (X1, X2)           |
| `fw-net`                                                            | Endpoints (address, port 0, inherited socket), bound-address reporting, TLS loading with rustls, socket-activation parsing                                                                                                                                                                                               | `Server::prepare` / `bind` (X6)             |
| `fw-http`                                                           | `Http`, `HttpCx`, the router, extractors, responses, SSE, the pre-dispatch stage (middleware and tower layers), CORS, WebSocket upgrade hand-off, the `Backend` SPI                                                                                                                                                      | `Transport`, `Server`, `Controller`/`Mount` |
| `fw-http-axum`, `-actix`, `-salvo`, `-poem`, `-rocket`              | One `fw_http::Backend` each                                                                                                                                                                                                                                                                                              | `fw_http::Backend`                          |
| `fw-ws`                                                             | `Ws` and `WsConnect`, gateways, sessions, the connection hook traits (`OnConnect`, `OnDisconnect`, `AfterInit`), the message envelope, rooms and broadcast, `BroadcastAdapter` SPI, a standalone WebSocket server for a separate port, with TLS                                                                          | `Transport` ×2, `Server`                    |
| `fw-ws-redis`                                                       | `BroadcastAdapter` over Redis pub/sub                                                                                                                                                                                                                                                                                    | `BroadcastAdapter`                          |
| `fw-rpc`                                                            | `Rpc`, `RpcCx`, the frame grammar, pattern routing, all four call shapes, `RpcClient`, the `Link` SPI                                                                                                                                                                                                                    | `Transport`, `Server`                       |
| `fw-rpc-tcp`, `-udp`, `-nats`, `-redis`, `-amqp`, `-mqtt`, `-kafka` | One `Link` each                                                                                                                                                                                                                                                                                                          | `Link`                                      |
| `fw-rpc-conformance`                                                | A shared test suite every `Link` runs                                                                                                                                                                                                                                                                                    | `Link`                                      |
| `fw-grpc`                                                           | `Grpc`, `GrpcCx`, the `Method` trait, the pre-dispatch stage (middleware and tower layers, with the HTTP-status translation), status and details mapping, reflection, health, client modules (on tonic)                                                                                                                  | `Transport`, `Server`                       |
| `fw-grpc-build`                                                     | `build.rs` code generation with protox (no system protoc) plus prost and tonic generators                                                                                                                                                                                                                                | —                                           |
| `fw-graphql`                                                        | Transport-neutral: the `Engine` SPI, `GqlRequest`, `GqlResponse`, the `GqlContext` convention, `fw_graphql::dep`                                                                                                                                                                                                         | `Execution`                                 |
| `fw-graphql-http`                                                   | The GraphQL-over-HTTP endpoint (a controller), its status rules, the playground, `GraphqlModule`                                                                                                                                                                                                                         | `fw-http`                                   |
| `fw-graphql-ws`                                                     | The graphql-transport-ws gateway                                                                                                                                                                                                                                                                                         | `fw-ws`                                     |
| `fw-graphql-async`, `fw-graphql-juniper`                            | One `Engine` each, depending on `fw-graphql` alone                                                                                                                                                                                                                                                                       | `Engine`                                    |
| `fw-tokio`                                                          | `Timer`, `shutdown_signal()`, spawn helpers                                                                                                                                                                                                                                                                              | `Timer`, `Signal`                           |
| `fw-dev` (installs `cargo fw`)                                      | Watch, rebuild, restart, socket holding                                                                                                                                                                                                                                                                                  | socket activation (`fw-net`)                |

Each transport crate re-exports its attribute macros from one proc-macro crate (`fw-http-macros` and so on). Each of those is a thin layer over `fw-handler-codegen`.

---

## 2. Common to every transport

### 2.1 Handlers and keys [1][12][13]

A handler is a method on a controller, marked by its transport's attribute. One `#[routes]` impl may mix transports. DESIGN §7 already checks each handler's enhancers against its own transport's roles.

**X1: transport keys.** `Transport` gains `const KEY: &'static str` (`"http"`, `"ws"`, `"ws_connect"`, `"rpc"`, `"grpc"`). The `__handler` protocol gains one obligation: a transport's attribute must emit, next to `fn __fw_mount_<name>`, an associated constant `const __FW_KEY_<name>: &'static str = <Http as Transport>::KEY;`. An attribute macro on an impl item may emit several items, so this fits the protocol as it stands.

`#[routes]` expands first. It knows every scoped key written at the controller level (the `htpp` in `#[guards(htpp = AuthGuard)]`) and every handler name, so it emits one assertion per key, as a free `const _` after the impl naming the controller type:

```rust
const _: () = assert!(
    ::fw::__private::key_in("htpp", &[UsersController::__FW_KEY_get, UsersController::__FW_KEY_get_rpc]),
    "`htpp` is not the key of any handler's transport in this impl (handlers: get, get_rpc)"
);
```

A free `const _` is evaluated unconditionally. Inside the impl it would need a name, and a named associated constant is evaluated only where it is read, so a misspelled key would compile unnoticed. Const evaluation runs after every attribute has expanded, so the `__FW_KEY_*` constants exist by then. The assertion is spanned on the key token, which makes a misspelling a compile error (E0080) pointing at `htpp`. A generic controller is the one case a free const cannot name: there `#[routes]` emits a named associated constant and the generated `Controller::mount` reads it (`let () = Self::__FW_KEYS_CHECK_htpp;`), which evaluates it when `mount` is instantiated. `key_in` compares bytes in a loop, since a trait method is not callable in a `const fn` on stable (X11). A misspelled transport key on a controller-level entry is therefore refused at compile time.

**X2: impl-level shared values.** A controller-level `value = expr` becomes legal again [13]. `#[routes]` generates `impl Controller`, and its `mount` builds each impl-level value **once**, into an `Arc`. It passes a generated `__FwShared<V0, V1, ..>` struct to every per-handler mount function, one type parameter per impl-level `value` entry in the order written across the three enhancer attributes, inferred by `mount` from the expressions, since `#[routes]` cannot name a value's type: `fn __fw_mount_<name><V0: Interceptor<Http>, ..>(m: &mut Mount<'_>, shared: &__FwShared<V0, ..>)`. Each per-handler function bounds the parameters it uses by the role for its own transport and leaves the others unbounded, so an `http(value = ..)` entry is a parameter an RPC handler's function never names, and a value lacking the role fails E0277 at that handler's mount call, which the macro spans at the handler's name. The position of each `value` entry is part of the `__handler` contract, as the turbofish order of `Contribute::try_singleton` already is; the transport attribute learns it from the controller-tier tokens it receives. `EnhancerSpec` gains `guard_arc(Arc<V>)`, `interceptor_arc` and `error_handler_arc`. Each handler unsizes its own clone of the same `Arc` into its transport's role, so the role check still runs per handler. A rate limiter declared on the impl now limits the controller as a whole. The compile error from wave 2 (M 1) is removed.

### 2.2 Parameters and extraction [2][3]

```rust
/// A handler parameter built from the call.
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be built from a `{T}` call",
    note = "a transport implements `FromCall` for its own extractors; `FromContainer` types (`Dep`, `Ext`, ...) are accepted on every transport"
)]
pub trait FromCall<T: Transport>: Sized + Send + 'static {
    /// Whether this parameter consumes the body or payload. At most one may.
    const CONSUMES_BODY: bool = false;
    /// What it reads from the container, if anything, for the wiring pass.
    fn dependencies(_d: &mut Dependencies) {}
    fn from_call(cx: &T::Cx) -> impl Future<Output = Result<Self, ExtractError>> + Send;
}
```

The pair `FromContainer` / `FromCall` names where a parameter comes from, the container or the call, and both appear in diagnostics. `fw-transport` implements `FromCall<T>` for every `T` on the core's own injection-point types: one concrete impl each for `Dep<U, Q>`, `Many`, `Ext`, `ModuleRef` and `ExecutionRef`, and one generic impl `impl<T, P: FromCall<T>> FromCall<T> for Option<P>` that forwards `CONSUMES_BODY`. These delegate to `FromContainer` and declare their reads, and the generated code records every parameter's reads on the handler's `HandlerSpec` (§2.5), so wiring checks handler parameters like any constructor's. A user-defined `FromContainer` type is wrapped as `Injected<S>`. The macro accepts it bare, because it probes concrete parameter types with autoref: rank one is `P: FromCall<T>`, rank two `S: FromContainer` as `Injected<S>`, the transport a parameter of the probe type rather than of the method, since lookup checks an impl's where-clauses and never a method's. `Option<Dep<U>>` takes rank one through the `Option` impl, and `Option<UserType>` arrives through rank two as `Injected<Option<UserType>>`, `Option<S: FromContainer>` being `FromContainer` in the core. Execution inputs are plain `Dep<RequestHead>`, and extension values are `Ext<CurrentUser>`. The context itself is a parameter, through `impl FromCall<Http> for HttpCx`.

**The single body consumer is checked at compile time and names both parameters.** For each pair of parameters, the macro emits one assertion, n(n−1)/2 of them for n parameters:

```rust
const _: () = assert!(
    !(<Json<NewUser> as FromCall<Http>>::CONSUMES_BODY && <Form<Login> as FromCall<Http>>::CONSUMES_BODY),
    "`user` and `login` both consume the body; a handler reads the body once"
);
```

It's spanned on the second parameter. No pair is skipped by spelling: `CONSUMES_BODY` is a property of the type, and `type Body<T> = Json<T>` consumes whatever it is called. The compile cost is one comparison per constant.

**Extraction failure has one shape everywhere**, flat, each variant carrying the parameter's name so one pattern matches it:

```rust
#[non_exhaustive]
pub enum ExtractError {
    Missing { param: &'static str },                                       // a required header, query field or path segment
    Malformed { param: &'static str, source: Redacted },                   // could not decode
    UnsupportedMediaType { param: &'static str, expected: &'static str },
    TooLarge { param: &'static str, limit: u64 },
    Invalid { param: &'static str, violations: Vec<FieldViolation> },      // validation rules failed
}

impl ExtractError { pub fn param(&self) -> &'static str; }
```

`Malformed`'s `source` is built through `AppHandle::redact` (X14), which runs the graph's own redaction over the decoder's error; the transport holds an `AppHandle` from `Mounted`. `ExtractError` implements `Classify` (§2.4), `BadRequest` or `Unprocessable` for `Invalid`, so it reaches the error handlers as a `CallError` of that kind through the blanket `From`, holding the `ExtractError` as its source. An error handler reshapes it with `err.downcast_ref::<CallError>()?.source_as::<ExtractError>()`. Each transport documents its rendering in §§3–7, and HTTP adds the exact statuses 413 and 415 from the variant.

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

`Valid<P>` is `impl<T, P: FromCall<T>> FromCall<T> for Valid<P>`, forwarding `CONSUMES_BODY` as `Option<P>` does. It runs `Validate::validate` after `P` extracts, and turns violations into `ExtractError::Invalid`. A bridge for the `validator` crate sits behind a feature flag, for teams that already use it.

### 2.3 Replies [4]

A handler returns `()`, a value, a stream, or any of those inside a `Result`. The error side always reaches the error handlers:

```rust
pub trait IntoReply<T: Transport>: Send + 'static {
    fn into_reply(self, cx: &T::Cx) -> Result<T::Reply, IntoReplyError>;
}
```

`IntoReplyError` is the conversion's own failure, a serializer refusing a value for example; it implements `Classify` as `Internal` and holds the cause as its source. Transports implement `IntoReply` for their reply types: `Json<T>`, `Sse<S>`, `pb::User`, `impl Stream`, and so on. For a `Result<V, E>`, the generated code doesn't match on the _spelling_. It probes the _type_ by autoref, at the concrete call site:

```rust
let out = Self::get(&this, a, b).await;              // whatever the return type is spelled as
(&&&::fw::__private::IntoReplyProbe::new(out)).into_reply::<Http>(&cx)
```

The probe has three arms. Every error the first arm accepts the second accepts too, a `CallError` being an `Error`, so the arms are not disjoint and the one reached first wins. Method lookup tries the impl on `&&IntoReplyProbe` first, then `&IntoReplyProbe`, then the bare type, so each arm sits one reference deeper than the priority order reads:

- `Result<V, E>` where `E: Into<CallError>`, on `&&IntoReplyProbe<Result<V, E>>`: `Err` becomes `BoxError::from(CallError::from(e))`. Through the blanket `From` (§2.4) this catches every `Classify` error, a `CallError` itself, and a user type with its own `From<MyErr> for CallError`.
- `Result<V, E>` where `E: Into<BoxError>`, on `&IntoReplyProbe<Result<V, E>>`: `Err` is boxed unchanged.
- Any `V: IntoReply<T>`, on `IntoReplyProbe<V>`: answered as the value.

The ranked methods take `&self`, so the value sits in a `Cell<Option<_>>`, as the core's factory probe already does. The compiler resolves a type alias (`type ApiResult<T> = Result<T, ApiError>`) before method resolution, so an alias takes the same arm as the plain `Result`. In the value API, `r.map_err(CallError::from)` does the same thing explicitly.

**Streaming returns and `'static`.** A reply outlives the handler call, and on edition 2024 an opaque return type captures `&self`'s lifetime whatever the hidden type borrows. The transport attribute appends `+ use<>` to every opaque type in a handler's return position, inside an `async fn`'s return type included, unless the handler has a type or const parameter or the opaque type already names a `use<..>` or a lifetime; those are left as written. A stream that borrows `self` then fails inside the handler body. The idiom for a stream that needs the controller is `self: Arc<Self>` as the receiver, a stable arbitrary self type, so the stream captures the `Arc` instead of borrowing: `async fn watch(self: Arc<Self>, ..) -> impl Stream<..>`. The generated call already holds the controller in an `Arc`.

**Errors in the middle of a stream (X7).** An `Err` from the outer `Result` goes through `dispatch`'s error handlers like any other error. `dispatch` returns the reply without pulling the stream, so an SSE handler's headers are never held back for its first event, and every item error, the first included, takes the late path: an `Err` _item_ cannot change a status. The core gains `fw::dispatch_late(handler, cx, err) -> LateOutcome`, which runs the same error handlers with `cx.exec().is_late() == true`:

- A handler returning `Err(e2)` reshapes the error, and the transport renders `e2` in its mid-stream form (§§3–6).
- A handler returning `Ok(_)` is ignored and logged, and the original error renders canonically. An `Ok` written for the pre-stream case is a failure rendering, and ending the stream cleanly on it would tell the client "complete" when the stream failed.
- A clean end is explicit: `Err(fw::EndStream)` ends the stream as if it had returned `None`.

### 2.4 One error model [5]

```rust
#[non_exhaustive]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ErrorKind {
    BadRequest, Unauthorized, Forbidden, NotFound, Conflict, Unprocessable,
    TooManyRequests, Timeout, Unavailable, Unimplemented, Internal,
}

/// A domain error that knows its transport-neutral kind.
pub trait Classify: std::error::Error + Send + Sync + 'static {
    fn classify(&self) -> ErrorKind;
    fn public_message(&self) -> Cow<'_, str> { self.to_string().into() }
    fn details(&self) -> Details { Details::default() }
}

/// The one concrete error every transport renders.
pub struct CallError { kind: ErrorKind, message: String, details: Details, source: Option<BoxError> }

impl CallError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self;
    pub fn unauthorized(challenge: impl Into<String>) -> Self;   // `Unauthorized` carrying its own `WWW-Authenticate` challenge
    pub fn from_boxed(err: BoxError) -> Self;                    // the recogniser, below
    pub fn with_details(self, d: Details) -> Self;
    pub fn kind(&self) -> ErrorKind;
    pub fn source_as<E: Error + 'static>(&self) -> Option<&E>;   // reach the domain error
}

impl<E: Classify> From<E> for CallError { /* keeps `e` as the source */ }
```

`#[derive(Classify)]` with `#[classify(not_found)]` is sugar for the trait impl; a derive's helper attribute is scoped to it, so `#[classify(..)]` sits beside thiserror's `#[error(..)]`. The blanket `From` is what makes `?` work: inside a function returning `Result<_, CallError>`, a `Classify` error converts itself. It compiles because **`CallError` never implements `Classify`**: with that impl the blanket would collide with std's `impl<T> From<T> for T` (E0119, "conflicting implementations of trait `From<CallError>` for type `CallError`"), which is why `CallError` carries an inherent `kind()` instead. The trait's documentation states the rule, since no `on_unimplemented` can carry it.

`Details` is a list of typed entries chosen to map one-to-one onto `google.rpc` error details, so gRPC gets them natively and the other transports get stable JSON:

```rust
#[non_exhaustive]
pub enum Detail {
    FieldViolations(Vec<FieldViolation>),      // google.rpc.BadRequest
    ErrorInfo { reason: String, domain: String, metadata: BTreeMap<String, String> },
    RetryAfter(Duration),                      // google.rpc.RetryInfo; HTTP Retry-After
    Help(Vec<Link>),
    Json(serde_json::Value),                   // anything else; gRPC carries it as a google.protobuf.Value packed in an Any
}
```

**Recognising a `BoxError` at render time.** `CallError::from_boxed(BoxError)` walks the error and maps the core's own errors. It is a named constructor rather than a `From<BoxError>` impl: coherence would accept the impl, since `Classify` is local and `Box<dyn Error>` can never implement it, but a recogniser that walks an error should not run invisibly on every `?`. It takes the error by value because that is what `source` stores and what the error handlers hand on.

| Error                                                    | Kind                                                                                                                       |
| -------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------- |
| `CallError`, bare or boxed by the reply probe            | its own                                                                                                                    |
| `ExtractError`                                           | `BadRequest` or `Unprocessable`                                                                                            |
| `GuardRejected`                                          | `Forbidden`                                                                                                                |
| `PanicRecovered`, and anything `fw::is_panic` recognises | `Internal`, with a generic message                                                                                         |
| `LookupError::Construct { reason: Errored(r) }`          | recurses into `r.downcast_ref::<CallError>()`, so a constructor's "tenant not found" becomes 404, as DESIGN §10.2 requires |
| `Closed`, `LookupError::Closed`                          | `Unavailable`                                                                                                              |
| anything else                                            | `Internal`, with the message withheld                                                                                      |

A passed deadline is not in the error: the reason lives on the execution (X5), so each transport tests `cx.exec().cancel_reason()` before calling `from_boxed` and renders `Timeout` for `CancelReason::Deadline`.

**Canonical renderings:**

| Kind            | HTTP                                                                                                                                                                                       | gRPC                                                 | WebSocket / RPC       |
| --------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ---------------------------------------------------- | --------------------- |
| BadRequest      | 400                                                                                                                                                                                        | INVALID_ARGUMENT                                     | `"bad_request"`       |
| Unauthorized    | 401, with a `WWW-Authenticate` challenge (RFC 9110 requires one): the server's configured default, `Bearer` unless set (RFC 6750), or the one `CallError::unauthorized(challenge)` carries | UNAUTHENTICATED                                      | `"unauthorized"`      |
| Forbidden       | 403                                                                                                                                                                                        | PERMISSION_DENIED                                    | `"forbidden"`         |
| NotFound        | 404                                                                                                                                                                                        | NOT_FOUND                                            | `"not_found"`         |
| Conflict        | 409                                                                                                                                                                                        | ABORTED                                              | `"conflict"`          |
| Unprocessable   | 422                                                                                                                                                                                        | INVALID_ARGUMENT, with `BadRequest` field violations | `"unprocessable"`     |
| TooManyRequests | 429, plus `Retry-After` from `RetryAfter`                                                                                                                                                  | RESOURCE_EXHAUSTED                                   | `"too_many_requests"` |
| Timeout         | 504                                                                                                                                                                                        | DEADLINE_EXCEEDED                                    | `"timeout"`           |
| Unavailable     | 503, plus `Retry-After` when a `RetryAfter` detail is present (RFC 9110 makes the header optional)                                                                                         | UNAVAILABLE                                          | `"unavailable"`       |
| Unimplemented   | 501                                                                                                                                                                                        | UNIMPLEMENTED                                        | `"unimplemented"`     |
| Internal        | 500                                                                                                                                                                                        | INTERNAL                                             | `"internal"`          |

HTTP bodies are RFC 9457 `application/problem+json`: `type` is `about:blank`, `title` the status phrase, `status` the code and `detail` the `public_message`, with `details` as an extension member. gRPC sends the status message plus a `google.rpc.Status` with packed details in `grpc-status-details-bin`; a `Detail::Json` travels as a `google.protobuf.Value`, which holds any JSON value, packed in an `Any`. WebSocket and RPC use the envelope `{ "kind", "message", "details" }`.

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

Metadata values are plain `Send + Sync + 'static` values, built once at mount. `meta` is in the `#[routes]` inert list: `#[routes]` parses it at both tiers and carries it in the protocol as `__handler(name, controller(..), method(..), meta(controller(..), method(..)))`, and a method carrying `#[meta]` alone is not a handler. `fw` exports a `meta` attribute macro whose body is the error "goes below `#[routes]`", so a stray `#[meta]` outside a `#[routes]` impl is a compile error.

**X3: handler information on the execution.** `Mount::handler` takes a `HandlerSpec<T, H>` builder instead of positional arguments, named as the core's `EnhancerSpec` is: `.controller(spec)`, `.method(spec)`, `.meta(Metadata)`, `.route(impl Into<Cow<'static, str>>)`, `.shape(Shape)` and `.dependencies(Dependencies)`. The generated code fills `.dependencies` from each parameter's `FromCall::dependencies`, and the wiring walk adds those reads to its roots as it adds a closure's, so a handler reading `Dep<RequestHead>` on an RPC controller, or `Dep<Foo>` its module cannot see, fails at `wire()` rather than at the first call. `route` takes a `Cow` so a path can be built at runtime. For a configured module the common need is a prefix: `ModuleDef::controller::<C>()` returns a handle with `.at(prefix)`, a runtime value applied to every route and gateway path of that controller, which covers GraphQL and health endpoints without exposing `Mount` closures. A case a prefix cannot express, per-route paths from configuration or one controller mounted twice, has no spelling; a `controller_with(closure)` exposing the mount is the form to add when one appears. `dispatch` sets the mounted handler's `HandlerInfo` on the execution before the first guard runs, so enhancers read it through `cx.exec().handler()`:

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

**X5: cancellation reasons.** The core gains `cancel_with(CancelReason)` on `Execution` and on `ExecutionRef`, since a disconnect is noticed in a task holding a clone, and `ExecutionRef::cancel_reason() -> Option<CancelReason>`. The reason sits in a write-once slot beside the cancellation signal. `cancel()` stays, as `cancel_with(CancelReason::Explicit)`, so a cancelled execution never answers `None`; the drain's end writes `Drain`:

```rust
#[non_exhaustive]
pub enum CancelReason { Disconnected, ClientCancelled, Deadline, Drain, Explicit }
```

Each transport fires the reason its wire tells it:

- `Disconnected`: the backend observed the peer close the connection or reset the stream, through a dropped response body, a closed connection or an h2 `RST_STREAM`. The server dropping an unread request body is not one.
- `ClientCancelled`: a WebSocket or RPC `cancel` frame, or a gRPC `RST_STREAM(CANCEL)`.
- `Deadline`: a passed `grpc-timeout`, RPC `deadline-ms` header, or configured route timeout.
- `Drain`: the core's end of the drain.
- `Explicit`: `cancel()` by whoever holds the execution.

**Clean end versus cut off.** Every streaming answer is wrapped in `fw_transport::Tracked<S>`, which records one of two outcomes:

- `Completed`: the stream returned `None` and the transport finished writing.
- `CutOff(reason)`: dropped before that.

The handler or an interceptor registers for the outcome with `cx.exec().on_stream_end(|outcome| ..)`, a callback the core runs synchronously when the reply stream finishes. It is runtime-free, needs no spawned task, and works from an interceptor, which returns the reply before the stream is consumed and so could not await it. The outcome is the reply stream's; an inbound stream's end is already visible to the handler that reads it. On the wire, each transport uses its protocol's own end marker:

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

`Endpoint::parse("0.0.0.0:0")`, `Endpoint::inherited("http")` (matched by `LISTEN_FDNAMES`), and `Endpoint::inherited_index(0)` cover the three cases. An inherited socket is only accepted when `LISTEN_PID` equals the process ID, which follows the systemd protocol. `fw-net` sets `FD_CLOEXEC` on every inherited socket and leaves `LISTEN_FDS`, `LISTEN_FDNAMES` and `LISTEN_PID` in the environment: in edition 2024 `std::env::remove_var` is `unsafe` because it races with other threads, and under `#[tokio::main]` the runtime's threads exist before user code runs. The protocol covers the stale case without it, since a child spawned later fails the `LISTEN_PID` check; the documentation says why the variables stay.

**X6: two-step binding.** `Server` gains `prepare(&mut self, Mounted<'_, T>) -> impl Future<Output = Result<(), BoxError>> + Send`. `listen()` changes its order to:

1. Call `prepare` on **every** server. That's route-table construction, duplicate checks, TLS loading and key/certificate matching, CORS validation, endpoint parsing, and checking that inherited sockets exist. Every failure is collected and reported together as `StartupError::Configure(ConfigureErrors)`, a new variant (`StartupError` is `#[non_exhaustive]`) mirroring `Wiring(WiringErrors)`, one entry per failure carrying `{ transport, source: Redacted }`; each transport's `prepare` answers one error listing its own failures, the way `WiringErrors` does. Nothing has bound yet, so a configuration error is always reported before a port conflict.
2. Call `bind` on each server in order. A server's `bind` is all-or-nothing for its own listeners: it closes any listener it opened before returning `Err`. `listen()` then closes the servers already bound, which it already does. So nothing is ever left half-bound.

`Server::bound(&self) -> Vec<BoundAddr>` reports actual addresses, and `App<Bound>::addresses()` gathers them, so port 0 reports the port the OS chose.

**TLS** uses rustls via `fw_net::Tls::from_pem_files(cert, key)` or `Tls::from_pem(..)`. Both are parsed in `prepare`, so a bad certificate, a key that doesn't match, or an unreadable file fails startup as `Configure`, never inside the serve loop. ALPN is set per transport (`h2` and `http/1.1` for HTTP, `h2` for gRPC). The standalone WebSocket server (`wss`) and the TCP link take the same `fw_net::Tls`. Broker links take their client libraries' TLS configuration. UDP has no `tls` method at all, because DTLS isn't supported, so trying it is E0599.

### 2.8 Load shedding [11]

`fw_transport::Admission` is a runtime-free semaphore built on `async-lock`, with two counters: one per server and one per connection where a connection exists. Over the limit, each transport refuses in its protocol's own way. §§3–6 list the refusals. Where a refusal carries `Retry-After`, the value is the server's `.shed_retry_after(..)`, one second unset.

### 2.9 Spans [14]

Each transport wraps `dispatch` in a `tracing` span, created by `fw_transport::span::call(..)`. Span names and attributes follow the OpenTelemetry semantic conventions:

- HTTP: the span is named `GET /users/{id}`, with `http.request.method`, `http.route`, `url.path`, `url.scheme` and `http.response.status_code`.
- RPC: `rpc.system` (`"fw"`), `rpc.method` (the pattern) and `messaging.system` for brokers.
- gRPC: the span is named `$package.$service/$method`, with `rpc.system = "grpc"`, `rpc.service`, `rpc.method` and `rpc.grpc.status_code`.

Every span also carries `fw.transport` (the key) and `fw.handler`.

### 2.10 Execution inputs without user imports

**X4: transports declare their own inputs.** `Transport` gains `fn inputs(d: &mut Inputs) {}`. The freeze calls it once per transport type, the first time a handler of that transport mounts. It records the inputs with `seeded_by` set to that transport, so users never import a module to declare inputs and no transport ships an input module. A declaration arriving by both paths, a module's `m.input::<T>().seeded_by::<Tr>()` and a transport's `inputs`, with the same `(key, seeder)` is one declaration rather than a duplicate, and a transport-declared input's origin prints as "declared by transport `Http`" rather than naming a module:

- HTTP: `RequestHead`, `ClientAddr`.
- WebSocket: `ConnectionInfo`, `UpgradeHead`, `SessionHandle` (§4.1).
- RPC: `CallHeaders`, `LinkInfo`.
- gRPC: `GrpcMetadata`, `PeerAddr`.

The per-handler input check from DESIGN §6.4 then works across transports unchanged. A transport whose handlers mount nowhere declares no inputs, so in such an app a non-optional `Dep<RequestHead>` reads as a missing dependency rather than an unseeded input.

**X8: routing to a module after the execution opens.** The pre-dispatch stage's unscoped entries (§3.3) run before the controller, and so before its module, is known. The core gains `Execution::route_to(&ModuleRef)`. It's callable once, by the holder only, before `dispatch`. The execution opens with root visibility, and after routing it switches to the controller's module. Extensions written before the switch survive it. Two consequences are visible to a middleware author: a resolver created before `route_to` keeps the root module, and a key that names a different binding under the controller's module is a second instance in the one execution, since instances sit in the execution cache under their binding.

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
    async fn events(self: Arc<Self>, last: LastEventId, cx: HttpCx) -> Sse<impl Stream<Item = Result<Event, UserError>>> {
        let resume = last.0;                                              // the stream captures the Arc rather than borrowing
        let exec = cx.exec().clone();                                     // the controller; the macro appends `+ use<>` (§2.3)
        Sse::new(self.feed.since(resume).take_until(exec.draining()))   // ends cleanly when the drain starts
            .keep_alive(Duration::from_secs(15))                          // ": keepalive" comments
    }
}

#[derive(Debug, thiserror::Error, Classify)]
pub enum UserError {
    #[error("user not found")] #[classify(not_found)] NotFound,
    #[error("email already registered")] #[classify(conflict)] Taken,
}
```

**`HttpCx`** is `Clone + Send + Sync`: an `ExecutionRef`, an `Arc<RequestHead>`, the matched route, path parameters, the client address, a take-once body slot, and an interior-mutable response-head writer. Its methods include `head()`, `method()`, `uri()`, `headers()`, `route()`, `exec()`, `ext::<T>()`, `take_body()`, `response_headers()` and `client_addr()`.

**Extractors** [17] are `Path<T>`, `Query<T>`, `Json<T>`, `Form<T>`, `Bytes`, `BodyStream`, `Multipart`, `Header<H>` (typed, from the `headers` crate), `HeaderMap`, `LastEventId`, `HttpCx`, and every container type. `Json`, `Form`, `Bytes`, `BodyStream` and `Multipart` consume the body. The body limit is a server setting (`.body_limit(2 * MB)`) that `#[meta(BodyLimit(..))]` can override per route. Bodies over the limit fail with 413 before deserialization.

**Responses** [18] are any `T: IntoReply<Http>`:

- `Json<T>`, `Bytes`, `String`, `()` (204), `Created<T>`, `NoContent`, and `(StatusCode, T)`.
- `WithHeaders<T>`, and `Response::builder()` for any status, headers and body.
- `Body::stream(s)` for a streaming body.
- `Sse<S>` with `Event::default().data(..).id(EventId).event(EventName).retry(..)`. Data is split into `data:` lines at CR, LF and CRLF alike, since a reader ends a line at any of the three. `EventId` and `EventName` are built fallibly (`EventId::new(..)?`) and refuse line terminators, and the id refuses U+0000, which a reader ignores, so a bad value fails where it is built and never mid-stream. Keep-alive comments are written as `: keepalive`, per the SSE spec. The content type is `text/event-stream` in UTF-8.

### 3.2 Routing [15]

`fw-http` owns the router, so every backend routes identically:

- Patterns use `{name}` segments plus an optional trailing `{*rest}`.
- A trailing slash is insignificant: both the pattern and the request path drop it, except for `/`.
- Routes are built in `prepare`. Two routes with the same method and the same normalized pattern, or two patterns that differ only in parameter names at one position (`/u/{id}` and `/u/{name}`), are a `Configure` error naming both handlers.
- `prepare` also checks every `Path<T>` against its route, by running `T`'s `Deserialize` impl against a deserializer that records what it asks for, so a mismatch is caught at startup rather than by the first request. A struct is checked by its field names, serde renames included; a tuple by its count; a scalar or newtype requires exactly one parameter. A map, a `#[serde(flatten)]` struct and a hand-written impl calling `deserialize_any` ask for no names and are not checked, which the documentation says.
- A path that matches nothing answers 404. A matching path with the wrong method answers 405 with an `Allow` header, which RFC 9110 requires.
- `HEAD` is answered from the `GET` handler with the body omitted, unless a `HEAD` handler exists (RFC 9110 requires general-purpose servers to support `HEAD`).
- `OPTIONS` without a handler answers 204 with `Allow`. CORS preflight never reaches routing (§3.4).

### 3.3 Pre-dispatch [19]

Middleware is HTTP-specific, so it isn't a core role. Middleware types are still ordinary container bindings, and the metadata that declares them reports its dependencies (`Meta::dependencies`), so wiring checks them.

```rust
pub trait Middleware: Send + Sync + 'static {
    fn handle(&self, req: Request, next: fw_http::middleware::Next<'_>) -> impl Future<Output = Response> + Send;
}
```

`fw_http::middleware::Next` holds the middleware chain and the request; the core's `Next<'a, T>` holds the interceptor chain. A module importing both qualifies one or writes `use .. as ..`, as std's `fmt::Result` and `io::Error` are imported by module. No alias ships, and the two differ in signature, so a wrong import fails at the type.

There is one middleware stage, **pre-dispatch**, declared in module metadata:

```rust
m.meta::<fw_http::PreDispatch>()
    .apply_value(Cors::new().allow_origin("https://app.example").allow_credentials(true))
    .apply::<RequestId>()                                           // unscoped: every request, misses included
    .layer(TraceLayer::new_for_http())                              // tower, unscoped (§3.4)
    .layer_for(["/files/*"], RequestBodyLimitLayer::new(50 * MB))   // tower, scoped by route pattern
    .apply_for::<ApiKeyAuth>(["/admin/*"]).exclude(["/admin/health"]);
```

The stage runs in two sub-steps, because a path rewrite has to happen before route matching while scoping by route pattern needs the match. Unscoped entries run first: they can rewrite the path with `req.set_path(..)`, they see misses, and they can answer without calling `next`, which is how CORS preflight and authentication work. Then the route is matched and `route_to` (X8) switches the execution to the controller's module. Then the entries whose pattern matches the route run. Then `dispatch` (guards, interceptors, handler, error handlers). To the user it is one stage in one order: the order written, with modules in collection order (DESIGN §3.2) where several declare entries. Both sub-steps run inside the error chain: an entry's `Err` reaches the error handlers as a `BoxError` and its panic as `PanicRecovered`, and on a miss only the global error handlers apply, since no handler matched. A request that arrives with no execution to open, during the drain, never reaches the stage (§10).

Each entry is resolved through the module that declared it, inside the request's execution: `Mounted::module_meta::<T>()` (X9) yields `(ModuleRef, Arc<T>)` in collection order, and `ModuleRef::with_execution(&ExecutionRef)` gives a resolver with that module's visibility, so a per-execution middleware is built once per request and a binding only its own module sees resolves as `wire()` checked it.

There is no per-module middleware. Auth for a group of routes is a pattern-scoped pre-dispatch entry, which runs before guards and can set `CurrentUser`; logging, metrics or transforming a module's responses is an interceptor; rejecting a request is a guard. What the stage cannot express is "every controller in this module" by naming the module: `.at(prefix)` (§2.5) with a pattern-scoped entry covers it. If a case needs the module named, the enhancer stack gains a module tier (global, module, controller, method), inside the error chain and with the existing tiers' semantics; middleware does not return for it.

### 3.4 CORS and tower [20]

`fw_http::Cors` follows the Fetch specification's CORS protocol. It answers preflights (`OPTIONS` with `Access-Control-Request-Method`) with 204 and the `Access-Control-Allow-*` headers, adds `Vary: Origin`, and never echoes `*` together with credentials. That last combination is forbidden by the spec, so it's refused in `prepare`.

`.layer(L)` accepts any `tower::Layer` over `fw_http::Service`, which is `Service<http::Request<HttpBody>, Response = http::Response<HttpBody>>`, and `.layer_for(patterns, L)` scopes one by route pattern. Layers are composed once, in `prepare`, never per request. An unscoped layer wraps the whole stage before matching, so it can rewrite and it sees misses. For scoped layers, `prepare` builds each route's own stack from the layers whose pattern matches it, in declaration order, and routes with the same set of layers share one stack; a matched request runs its route's prebuilt stack. Layers run inside the pre-dispatch stage, so they behave the same on every backend, including actix and rocket, which aren't tower-based, and they sit inside the error chain: a layer's `Err` reaches the error handlers as a `BoxError` and its panic as `PanicRecovered`. A response a layer builds itself, tower-http's auth answering 401 for example, is a response and not an error, so error handlers don't see it; that is inherent to tower, and the documentation says so. `.layer` and `.layer_for` exist on the HTTP and gRPC stages alone (§6.2). The WebSocket and RPC stages take `Middleware<T>` only, their requests not being HTTP, so a layer on them is E0599.

### 3.5 WebSocket upgrades [21]

When a gateway's path matches a request carrying `Upgrade: websocket`, the router hands the request to `fw-ws` through the backend's upgrade future (§3.7). `fw-ws` completes the RFC 6455 handshake and runs the connection on the upgraded I/O. For a separate port, `fw_ws::Server::new(endpoint)` runs its own minimal HTTP/1.1 upgrade server, with `fw_net::Tls` for `wss`. Gateways are identical on either.

### 3.6 Wire behavior, limits, shutdown

- **Success:** the answer's status, headers and body. Streaming bodies use chunked encoding on HTTP/1.1 and data frames on HTTP/2.
- **Failure:** RFC 9457 problem details (§2.4). Extraction failures are 400, 413, 415 or 422 depending on the `ExtractError` variant, with field violations in `details`.
- **Load shedding:** over the server's in-flight limit, a request gets 503 with `Retry-After` set to `.shed_retry_after(..)`, one second unset. On HTTP/2, `SETTINGS_MAX_CONCURRENT_STREAMS` bounds each connection, and excess streams are refused with `RST_STREAM(REFUSED_STREAM)` per RFC 9113.
- **Cancellation:** the peer closing the connection or resetting the stream before the response has ended fires `Disconnected`. A route timeout (`#[meta(Timeout(..))]`) fires `Deadline` and renders 504.
- **Shutdown:** at `drain` the backend stops accepting, sends HTTP/2 GOAWAY, closes idle HTTP/1.1 keep-alive connections, and marks busy ones `Connection: close` on their next response. SSE handlers observe `draining()` and end their streams. A request that still arrives once `Execution::open` is refused, on a keep-alive connection before its `Connection: close` response, is answered directly with 503, `Retry-After` and `Connection: close`: no execution exists, so the pre-dispatch stage does not run for it. `close` closes everything that's left.

### 3.7 Backend SPI [16], written without macros

```rust
pub trait Backend: Send + Sync + 'static {
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
    pub body: HttpBody,                              // stream of Bytes; dropping it unread is not a disconnect (§2.6)
    pub conn: ConnInfo,                              // peer address, TLS info, protocol version
    pub upgrade: Option<OnUpgrade>,                  // resolves to an AsyncRead + AsyncWrite + Send + Unpin stream
}
```

`fw_http::Server<B: Backend>` implements the core `Server`. The `http` crate's types are the common language, and actix and rocket convert at their edge. A backend crate is roughly 300 lines of conversion plus the drain mapping.

`BackendLimits` is enforced in `prepare`: asking a backend for something it declares it can't do is a `Configure` error naming the limit. One entry is `upgrades`: a backend that cannot hand over a per-request upgraded I/O declares `upgrades: false`, and `prepare` refuses a gateway on its port. The table below is the starting point, and the conformance tests confirm each entry before release:

| Backend             | Documented limits                                                                                                                                                                                                                                                                                            |
| ------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| axum (hyper)        | none expected                                                                                                                                                                                                                                                                                                |
| salvo, poem (hyper) | none expected                                                                                                                                                                                                                                                                                                |
| actix-web           | HTTP/2 only over TLS through ALPN, so h2c is refused; an HTTP/1 upgrade is a service-level hook (`HttpService::upgrade`) rather than a per-request future, so the backend wires `svc` through that hook or declares `upgrades: false`; its multi-runtime model means the `Send` futures run on actix workers |
| rocket              | inherited sockets and port 0 depend on the version's custom-listener support, and are refused where missing; graceful-shutdown timing follows rocket's own shutdown configuration                                                                                                                            |

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
    #[fw_ws::message("chat.send")]
    #[guards(value = RateLimit::per_second(5))]
    async fn send(&self, msg: Payload<ChatMessage>, session: Session<ChatSession>, rooms: Dep<Rooms>)
        -> Result<Ack, ChatError>
    {
        rooms.to_room("lobby").except([session.conn_id()]).emit("chat.message", &msg.0).await?;
        Ok(Ack::default())
    }

    #[fw_ws::message("chat.history")]
    async fn history(self: Arc<Self>, q: Payload<HistoryQuery>) -> impl Stream<Item = Result<ChatMessage, ChatError>> {
        self.store.page(q.0)                                   // captures the Arc; the macro appends `+ use<>` (§2.3)
    }
}

impl fw_ws::OnConnect for ChatGateway {                        // after the connect guards; may refuse
    async fn on_connect(&self, cx: &ConnectCx) -> Result<(), ConnectRefused> {
        cx.conn().join("lobby").await;                         // `cx.ext::<CurrentUser>()` reads what the guards wrote
        Ok(())
    }
}

impl fw_ws::OnDisconnect for ChatGateway {
    async fn on_disconnect(&self, conn: &Connection, why: DisconnectReason) { self.presence.leave(conn.id()).await; }
}

impl fw_ws::AfterInit for ChatGateway {
    async fn after_init(&self, gw: GatewayRef) { tracing::info!(path = gw.path(), "chat ready"); }
}
```

`#[fw_ws::gateway]` sits on the impl next to `#[routes]`. `#[routes]` passes it through untouched, and it then emits `impl fw_ws::GatewayConfig`. The connection hooks are traits on the gateway type, `OnConnect`, `OnDisconnect` and `AfterInit`, following the core's rule that a role comes from the traits a type implements: they are not handlers, receive no `__handler`, and impl-level enhancers do not reach them; an impl-level `#[guards]` applies to message handlers alone.

**Two transports.** `Ws` covers message handlers (one execution per message). `WsConnect` covers the connection phase, with key `"ws_connect"`: its `Cx` is `ConnectCx` and its reply is admit or refuse. `fw-ws` mounts the connect handler itself, one per gateway, with the gateway attribute's `connect_guards(..)` as its guards; its body calls `OnConnect` when the type implements it, which `fw-ws` detects by probing the concrete type. Connect guards are ordinary `Guard<WsConnect>` implementations, running through `dispatch` like any guard, so refusals, error handlers, kinds and panics all work the same at connect time. A connection isn't an execution. The connect phase and each message are; `on_disconnect` runs as a terminal execution without guards, its failures and panics logged; `after_init` is a once-per-gateway lifecycle call, not an execution.

`#[routes]` doesn't know the impl is a gateway, so each `fw_ws::message` mount function calls `<Self as GatewayConfig>::mount_gateway(m)` under **X15, `Mount::once::<K>(..)`**, which makes the call idempotent per controller type. A gateway with no message handlers would mount nothing, and the gateway attribute refuses that case at compile time.

**Sessions** [24]: when the upgrade completes, the session is created before the connect guards run, from `Default` or from a factory declared with `session_with = |head: Dep<UpgradeHead>| ..`. `Session<T>` is a `FromContainer` type whose `describe` declares a third WebSocket input, `SessionHandle`, which `fw-ws` seeds into every connection-scoped execution: the connect phase, every message and `on_disconnect`. The session lives until the last execution holding it ends, so it is readable in `on_disconnect`. Per-message state lives in the message's execution.

**Connection hooks** [25]:

- `OnConnect::on_connect` runs after the connect guards and can refuse with a `ConnectRefused`.
- `OnDisconnect::on_disconnect` receives a `DisconnectReason`: `ClientClose { code, reason }`, `ServerClose { code }`, `ProtocolError`, `Drain`, or `Lost`. During the drain it runs as a terminal execution (DESIGN §6.3), so it's best effort at shutdown.
- `AfterInit::after_init` runs once per gateway after `listen()`, with a `GatewayRef`.

**Where a refusal happens.** Connect guards run after the 101 handshake and refuse with a close code (§4.2), the one path that tells a browser why: a browser `WebSocket` cannot read the handshake's HTTP status, and sees a refused upgrade as a generic `error` and a close with 1006, while a close after 101 reaches the script as `CloseEvent.code` and `reason`. A gateway written `refuse = handshake` refuses before the upgrade instead, with 401 or 403 as RFC 6455 §4.2.2 allows, for non-browser clients and reverse-proxy access logs; such a refusal counts against no connection limit.

### 4.2 Wire format

The message envelope is defined here, since no specification covers one. Text frames carry JSON. Binary frames carry MessagePack, configured per gateway with `codec = msgpack`. The event field name is configurable, and graphql-ws uses `type`.

```
→ {"event":"chat.send","id":7,"data":{...}}
← {"id":7,"data":{...}}                                  single answer (ack)
← {"id":7,"data":{...}} ... {"id":7,"complete":true}     streamed answer
← {"id":7,"error":{"kind":"forbidden","message":"...","details":[...]}}
→ {"event":"cancel","id":7}                              fires ClientCancelled
```

A message without an `id` is fire-and-forget: success sends no ack, and a failure answers the `error` envelope without an `id`, so a guard's refusal stays visible to the client. Control frames (ping, pong, close) are answered by the protocol layer and never reach handlers [26]. Every failure is the one `error` envelope: extraction, a guard, a handler, a panic, or an unknown event (kind `unimplemented`).

**Close codes** [23], from the IANA WebSocket Close Code Number Registry that RFC 6455 established (RFC 6455 itself defines 1000–1011 and 1015; 1013 is a registry entry):

- A connect refusal by kind: `Unauthorized` and `Forbidden` close with 1008 (policy), `TooManyRequests` with 1013 (try again later), `Internal` with 1011, and `Unavailable` with 1013.
- A gateway may map kinds to its subprotocol's own codes, as graphql-transport-ws does with 4401, 4403 and 4429. `ConnectRefused::code(4403, "forbidden")` sets one explicitly.
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

The call shape comes from the signature: an `Inbound<T>` parameter means a streamed request, and a stream return means a streamed reply. The shape is recorded on the `HandlerSpec` (X3). A streamed reply that needs the controller takes `self: Arc<Self>` (§2.3).

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

**A pattern nothing handles** is `Unavailable` on every link, with an `ErrorInfo` whose `reason` says what the link knows. TCP and UDP answer server-side, since the server receives the frame: `err` of kind `unavailable`, `reason: "pattern_unhandled"`, message "no handler for pattern `x`". On the brokers the subscription, queue or topic is the routing table, so a request for an unhandled pattern never reaches this server, and `RpcClient` maps each link's "no destination" signal to the same kind with `reason: "no_destination"`: NATS no-responders (a client-side 503 status since NATS 2.2), a Redis `PUBLISH` receiver count of zero, an AMQP `basic.return` on a `mandatory` publish, a Kafka `UNKNOWN_TOPIC_OR_PARTITION`. None of those signals tells "no handler" from "server down", which is why the kind is `Unavailable` and not `Unimplemented`; the latter would be false whenever the server is down. The conformance suite asserts the kind and accepts either reason. An unhandled _event_ is logged and counted, and on brokers it's acknowledged as rejected, so it can't loop on redelivery (AMQP: `basic.reject` without requeue, which routes to a dead-letter exchange if one is configured). A shape mismatch, such as `req` sent to a streamed-request pattern, answers `bad_request`.

**What a caller sees is uniform.** `RpcError` exposes a kind from §2.4 and maps link-level failures the same way everywhere: no responders, a lost link, or a broker refusal map to `Unavailable`, a client timeout maps to `Timeout`, and an oversized payload maps to `BadRequest` with `ErrorInfo { reason: "payload_too_large" }`. `fw-rpc-conformance` runs one scenario list against every link (unary, each streaming shape, cancel mid-stream, unknown pattern, deadline, binary payload, oversized payload, drain) and asserts the same caller-visible result.

**Binary payloads.** `Bytes` and `Payload<Bytes>` travel raw inside CBOR envelopes. A link configured with a JSON codec, chosen for interoperability, declares `binary: false`. A handler with a binary payload on such a link is refused in `prepare`, from the shape recorded on its `HandlerSpec`, with a `Configure` error naming the pattern and the link. A client call with one is a runtime value `prepare` cannot see; it fails before any I/O with an `RpcError` of kind `BadRequest` and `ErrorInfo { reason: "binary_unsupported" }`.

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

| Link                  | Request-reply mapping                                                | Limits                                                                                                                                                                                                                      |
| --------------------- | -------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| TCP                   | multiplexed by `id` over one connection, with length-prefixed frames | ordered per connection; all shapes; TLS through `fw_net::Tls`                                                                                                                                                               |
| UDP                   | one frame per datagram, replies sent to the sender's address         | unordered, no delivery guarantee, payload at most 65,507 bytes minus the envelope; **unary and events only**, streamed shapes refused at startup; no TLS                                                                    |
| NATS                  | the reply subject (`_INBOX`); stream items on the inbox              | at-most-once; ordered per publisher and subject; maximum payload read from the server's `INFO`; no-responders status maps to `Unavailable`                                                                                  |
| Redis                 | pub/sub on `pattern`; replies on a per-client reply channel          | at-most-once; ordered per channel                                                                                                                                                                                           |
| RabbitMQ (AMQP 0-9-1) | `reply_to` plus `correlation_id` properties                          | at-least-once with ack after the handler completes, so **handlers must be idempotent**; ordered per queue with a single consumer; prefetch (`basic.qos`) bounds deliveries per channel, or per consumer with `global=false` |
| MQTT v5               | the v5 Response Topic and Correlation Data properties                | QoS configurable; ordered per topic and QoS; maximum packet size from CONNACK                                                                                                                                               |
| Kafka                 | a reply topic plus a correlation header                              | ordered per partition; the partition key is a caller-supplied ordering key, the client instance's ID unset, so one caller's requests stay ordered; high latency for request-reply; size limit from broker configuration     |

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

The client also offers `emit(pattern, &data)`, `stream(pattern, &req)` (returns a stream), `send_stream` and `duplex`. A timeout uses the app's `Timer`. Dropping a stream or a pending request sends `cancel`. There is no ambient execution, so a call made inside one says so: `.within(&exec)` forwards the remaining deadline as `deadline-ms` and cancels the call when the execution is cancelled.

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

- **Pre-dispatch:** gRPC is HTTP/2, `http::Request` and `http::Response` are already its types and tonic is built on tower, so `m.meta::<fw_grpc::PreDispatch>()` takes `.apply`, `.apply_for`, `.layer` and `.layer_for` as HTTP's does (§3.3, §3.4), scoped by method path (`/users.v1.UserService/*`). A layer that answers with a non-200 HTTP status is not a valid gRPC response, so the transport translates such a response with the gRPC spec's HTTP-to-status mapping (401 to UNAUTHENTICATED, 403 to PERMISSION_DENIED, 429 and 502–504 to UNAVAILABLE, and so on), and the client receives a proper `grpc-status`.
- **Deadlines:** `grpc-timeout` is parsed with the spec's units (`H`, `M`, `S`, `m`, `u`, `n`) into the execution deadline. When it passes, the execution is cancelled with `Deadline` and the call answers DEADLINE_EXCEEDED.
- **Errors:** kinds map to canonical codes per §2.4. `Details` are packed into a `google.rpc.Status` and sent in `grpc-status-details-bin`; a `Detail::Json` is a `google.protobuf.Value` in an `Any`.
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

The channel connects lazily (tonic's `connect_lazy`), so `connect` does no network I/O. `fw_grpc::outgoing(&exec, request)` sets `grpc-timeout` from the execution's remaining deadline; there is no ambient execution, so nothing forwards it on its own.

---

## 7. GraphQL [37]

```rust
type ApiSchema = Schema<QueryRoot, EmptyMutation, SubscriptionRoot>;

#[module(
    imports   = [GraphqlModule::for_root(GraphqlConfig::at("/graphql").subscriptions("/graphql/ws"))],
    providers = [
        with = |q: Dep<QueryRoot>, s: Dep<SubscriptionRoot>| Schema::new(q, EmptyMutation, s),   // the schema, `Auto`
        AsyncGraphql<ApiSchema> as dyn Engine,                                                  // the engine, under the SPI key
    ],
)]
pub struct ApiModule;

#[injectable(execution)]                                    // built per execution [37]
pub struct GqlContext { user: Option<Ext<CurrentUser>>, loaders: Dep<Loaders> }
```

GraphQL names no transport: the specification defines the language, validation and execution, and GraphQL-over-HTTP and graphql-transport-ws are two bindings. The crates follow that split. `fw-graphql` is transport-neutral, holding the `Engine` SPI, `GqlRequest: Deserialize`, `GqlResponse: Serialize`, the `GqlContext` convention and `fw_graphql::dep`; the two engine adapters depend on it alone, each a `Construct` type reading its schema (`AsyncGraphql<S>` reads `Dep<S>`), bound under `dyn Engine` as above so a handler on any transport takes `Dep<dyn Engine>`. `fw-graphql-http` holds the endpoint, the playground and the GraphQL-over-HTTP status rules, and its `GraphqlModule` is the module imported above; `fw-graphql-ws` holds the graphql-transport-ws gateway. An RPC-only application serves the same engine with an ordinary handler, `#[fw_rpc::message("gql")] async fn gql(&self, req: Payload<GqlRequest>, engine: Dep<dyn Engine>) -> GqlResponse`, and `subscribe`'s stream over RPC's streamed-reply shape.

The engine SPI, written without macros:

```rust
pub trait Engine: Send + Sync + 'static {
    fn execute(&self, req: GqlRequest, exec: ExecutionRef) -> BoxFuture<'static, GqlResponse>;
    fn subscribe(&self, req: GqlRequest, exec: ExecutionRef) -> BoxStream<'static, GqlResponse>;
    fn sdl(&self) -> String;
}
```

Each engine adapter builds the schema context per execution through `exec.get::<GqlContext>()`. async-graphql receives it as request `Data`, and juniper as its `Context`. Resolvers reach other services through `fw_graphql::dep::<T>(ctx)`.

- **HTTP:** the endpoint is a controller in `fw-graphql-http`, mounted `.at(config.path)` (§2.5), so role keys and inputs work as for any route. It follows GraphQL-over-HTTP: it accepts POST with `application/json` and GET for queries, and answers `application/graphql-response+json`, using request-error status codes as that spec defines. HTTP guards, interceptors and pre-dispatch entries apply, because the endpoint is an HTTP route.
- **Subscriptions:** graphql-transport-ws on an `fw-ws` gateway with `event = "type"`. The protocol's own messages (`connection_init`, `connection_ack`, `subscribe`, `next`, `error`, `complete`, `ping`, `pong`) are handled by the gateway, along with its close codes (4400, 4401, 4403, 4408, 4409, 4429).
- **Playground:** GraphiQL is served on GET with `Accept: text/html` in builds with `debug_assertions`, and never in release builds unless enabled explicitly.

---

## 8. Runtime [38]

`fw-tokio` provides three things:

- `Timer`: sleeps through `tokio::time`, and `now()` through `tokio::time::Instant::now().into_std()`, so a paused test clock drives deadlines and sleeps together, as DESIGN §3.9 requires.
- `shutdown_signal()`: SIGINT and SIGTERM on Unix, plus Ctrl-C and Ctrl-Close on Windows. It resolves to `Signal::new("SIGTERM")` and so on.
- `spawn(fut)` and `spawn_in(&exec, fut)`. The second holds an `ExecutionRef` for the task's lifetime and stops the task on the execution's cancellation.

---

## 9. Development command [39]

`cargo fw dev` watches the source (`notify`), rebuilds (`cargo build`), and restarts. When the app's endpoints include `Endpoint::inherited(..)`, which `fw dev` enables automatically through `FW_DEV=1`, the command binds those listening sockets **itself** and passes them to each child using the socket-activation protocol: `LISTEN_FDS`, `LISTEN_FDNAMES` and `LISTEN_PID`. It sets `LISTEN_PID` through a small exec trampoline, so the variable equals the child's process ID. In the child, `fw-net` sets `FD_CLOEXEC` on each inherited socket and leaves the three variables set (§2.7), so a subprocess the app spawns inherits neither the sockets nor a `LISTEN_PID` that matches it.

The restart sequence is:

1. Build the new binary while the old one keeps serving.
2. Signal the old child, which drains gracefully.
3. Start the new child on the same sockets.

While no child is accepting, the kernel queues connections in the listen backlog, which `fw dev` keeps open. Clients see a delay, never a refused connection. Socket holding is Unix-only. On Windows the command restarts without holding sockets, so a client can see a refused connection during a restart, and the documentation says so; a Windows handoff (`WSADuplicateSocket` into the child) is the path to add once the Unix one is built.

---

## 10. Shutdown, per transport [8]

All transports follow DESIGN §9.5. Here's what each does at each step:

| Step                                   | HTTP                                                                              | WebSocket                                                                     | RPC                                                                          | gRPC                                   |
| -------------------------------------- | --------------------------------------------------------------------------------- | ----------------------------------------------------------------------------- | ---------------------------------------------------------------------------- | -------------------------------------- |
| before-shutdown                        | serving normally                                                                  | serving normally                                                              | serving normally                                                             | health still SERVING                   |
| `drain` (stop accepting)               | GOAWAY (h2); idle keep-alives closed (h1); busy ones get `Connection: close`      | idle connections closed with 1001; busy ones stop reading                     | TCP `goaway`; NATS drain; AMQP `basic.cancel`; MQTT unsubscribe; Kafka pause | GOAWAY; health switches to NOT_SERVING |
| drain window                           | requests finish; SSE ends on `draining()`                                         | in-flight messages finish, then 1001; `on_disconnect` as a terminal execution | in-flight calls finish; streams end on `draining()`                          | in-flight calls finish                 |
| a call arriving once `open` is refused | 503, `Retry-After`, `Connection: close`, answered directly: no pre-dispatch stage | nothing: busy connections have stopped reading                                | TCP: `err` of kind `unavailable`                                             | UNAVAILABLE                            |
| cancel at timeout                      | `Drain` reason; connections reset                                                 | close 1001                                                                    | `err` of kind `unavailable`                                                  | CANCELLED/UNAVAILABLE per stream       |
| `close`                                | listeners and connections closed                                                  | sockets closed                                                                | links closed, offsets committed                                              | listeners closed                       |

---

## 11. SPI extensions (summary)

| #   | Extension                                                                                                                                                                                                                 | Where                                          | For                                                  |
| --- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------- | ---------------------------------------------------- |
| X1  | `Transport::KEY`; transport attributes emit `__FW_KEY_<name>`; `#[routes]` asserts scoped keys at compile time in a free `const _` after the impl, or a named associated const read from `mount` for a generic controller | `Transport`, `__handler` protocol              | [12], refuses a misspelled key                      |
| X2  | Impl-level values built once in `Controller::mount`, passed as `&__FwShared<V0, ..>`, one type parameter per `value` entry in writing order; `EnhancerSpec::{guard,interceptor,error_handler}_arc`                        | `__handler` protocol, `EnhancerSpec`           | [13], lifts the controller-level `value` refusal     |
| X3  | `HandlerSpec` builder for `Mount::handler` (metadata, route as a `Cow`, shape, dependencies); `Execution::handler() -> Option<&HandlerInfo>`; `AppHandle::handlers()`; `ModuleDef::controller::<C>().at(prefix)`          | `Mount`, `Execution`, `AppHandle`, `ModuleDef` | [6][14]; handler parameters in the wiring pass       |
| X4  | `Transport::inputs(d: &mut Inputs)`, declared at freeze on first mount; a module's declaration with the same `(key, seeder)` counts once; a transport origin on the declaration                                           | `Transport`                                    | input declarations without user imports              |
| X5  | `CancelReason`, `Explicit` included; `cancel_with` on `Execution` and `ExecutionRef`, `cancel()` as `cancel_with(Explicit)`; `cancel_reason`; `on_stream_end(callback)`, driven by `Tracked<S>`                           | `Execution`                                    | [7]                                                  |
| X6  | `Server::prepare` before any bind; `StartupError::Configure(ConfigureErrors)`; `Server::bound`, `App<Bound>::addresses()`                                                                                                 | `Server`, `listen()`                           | [9][10][15]                                          |
| X7  | `dispatch_late` and `is_late()`, for errors after a stream has started; `EndStream`                                                                                                                                       | `dispatch`                                     | [4][26][29]                                          |
| X8  | `Execution::route_to(&ModuleRef)`, once, holder-only, before `dispatch`                                                                                                                                                   | `Execution`                                    | [19], the unscoped pre-dispatch sub-step             |
| X9  | `Mounted::module_meta::<T>() -> (ModuleRef, Arc<T>)` in collection order; `ModuleRef::with_execution(&ExecutionRef)`                                                                                                      | `Mounted`, `ModuleRef`                         | [19], resolving pre-dispatch entries in their module |
| X11 | `fw::__private::key_in` (a const fn comparing bytes)                                                                                                                                                                      | core private                                   | X1                                                   |
| X12 | `GuardRejected` to `Forbidden` and `PanicRecovered` to `Internal` in `CallError::from_boxed`; no core change, listed because the mapping depends on core types                                                            | `fw-transport`                                 | [5]                                                  |
| X13 | `AppHandle::phase()` (Running, Stopping, Draining, Destroying, Closed)                                                                                                                                                    | `AppHandle`                                    | gRPC health; diagnostics                             |
| X14 | `AppHandle::redact(BoxError) -> Redacted`, the graph's own redaction for code outside the core                                                                                                                            | `AppHandle`                                    | `ExtractError::Malformed` (§2.2)                     |
| X15 | `Mount::once::<K>(..)`, idempotent per controller type                                                                                                                                                                    | `Mount`                                        | the gateway's connect handler (§4.1)                 |

X10, a stream-outcome future, is folded into X5's `on_stream_end`, and its number is not reused. X12 needs no core change.

---

## 12. What is refused, and where

| Refusal                                                                                                                                                                                                      | When                           | Mechanism                                                                                                                                                       |
| ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Two parameters that consume the body                                                                                                                                                                         | compile                        | const assertion naming both (§2.2)                                                                                                                              |
| A parameter type not buildable on the handler's transport (`Json<T>` on an RPC handler)                                                                                                                      | compile                        | `FromCall<T>` + `on_unimplemented`                                                                                                                              |
| A return type with no `IntoReply` for the transport (`Sse` on gRPC)                                                                                                                                          | compile                        | `IntoReply<T>` bound                                                                                                                                            |
| A controller-level transport key that matches no handler (`htpp`)                                                                                                                                            | compile                        | X1 const assertion, spanned on the key                                                                                                                          |
| An enhancer lacking its role for a handler's transport                                                                                                                                                       | compile                        | as in DESIGN §7                                                                                                                                                 |
| A gRPC handler whose types or shape don't match its `Method`                                                                                                                                                 | compile                        | trait bounds on the marker                                                                                                                                      |
| An invalid route pattern literal (`/u/{id`)                                                                                                                                                                  | compile                        | macro span error                                                                                                                                                |
| TLS on UDP                                                                                                                                                                                                   | compile                        | no method (E0599)                                                                                                                                               |
| A malformed `#[meta]`, `#[fw_ws::gateway]` or handler attribute                                                                                                                                              | compile                        | macro span error                                                                                                                                                |
| `#[meta]` outside a `#[routes]` impl                                                                                                                                                                         | compile                        | the `meta` attribute macro's own error                                                                                                                          |
| A gateway with no message handlers                                                                                                                                                                           | compile                        | `#[fw_ws::gateway]` span error (§4.1)                                                                                                                           |
| A tower layer on the WebSocket or RPC stage                                                                                                                                                                  | compile                        | no method (E0599); `.layer` exists on HTTP and gRPC alone                                                                                                       |
| Duplicate route, or conflicting parameter names at one position                                                                                                                                              | startup (`listen` → `prepare`) | `StartupError::Configure` naming both handlers                                                                                                                  |
| A `Path<T>` whose field names or tuple count don't match the route's parameters, or a scalar or newtype on a route with other than one; a map, a flattened struct or a `deserialize_any` impl is not checked | startup (`prepare`)            | recording deserializer (§3.2)                                                                                                                                   |
| CORS with `*` and credentials                                                                                                                                                                                | startup (`prepare`)            | Fetch specification rule                                                                                                                                        |
| A bad certificate, a mismatched key, an unreadable TLS file                                                                                                                                                  | startup (`prepare`)            | rustls parse                                                                                                                                                    |
| A bad endpoint, a missing inherited socket, `LISTEN_PID` mismatch                                                                                                                                            | startup (`prepare`)            | `fw-net`                                                                                                                                                        |
| A backend asked for something its limits forbid (h2c on actix; a gateway on a backend declaring `upgrades: false`)                                                                                           | startup (`prepare`)            | `BackendLimits`                                                                                                                                                 |
| Two gateways on one path, two handlers for one RPC pattern or gRPC path                                                                                                                                      | startup (`prepare`)            | `Configure`                                                                                                                                                     |
| A streamed shape on a link that can't carry it (UDP)                                                                                                                                                         | startup (`prepare`)            | `Capabilities`                                                                                                                                                  |
| A handler with a binary payload on a text-codec link                                                                                                                                                         | startup (`prepare`)            | `Capabilities`, from the shape on the `HandlerSpec`                                                                                                             |
| A client call with a binary payload on a text-codec link                                                                                                                                                     | runtime, before any I/O        | `RpcError` of kind `BadRequest`, `reason: "binary_unsupported"`                                                                                                 |
| Port already in use                                                                                                                                                                                          | startup (`bind`)               | `StartupError::Bind`, reported only after every `prepare` passed; nothing left half-bound                                                                       |
| A non-optional input read on a path from a transport that doesn't seed it                                                                                                                                    | startup (`wire`)               | DESIGN §6.4, with inputs declared through X4                                                                                                                    |
| A pre-dispatch entry whose dependencies are missing                                                                                                                                                          | startup (`wire`)               | metadata declares its dependencies                                                                                                                              |
| Route miss, wrong method                                                                                                                                                                                     | runtime                        | 404, or 405 with `Allow`                                                                                                                                        |
| Extraction failure                                                                                                                                                                                           | runtime                        | `ExtractError` through the error handlers, rendered per transport                                                                                               |
| Unknown WebSocket event                                                                                                                                                                                      | runtime                        | `unimplemented` envelope                                                                                                                                        |
| A pattern no RPC handler reaches                                                                                                                                                                             | runtime                        | `Unavailable`, `reason: "pattern_unhandled"` (TCP, UDP: the server's `err`) or `"no_destination"` (brokers: the client's mapping); one kind across links (§5.2) |
| A call arriving once `Execution::open` is refused                                                                                                                                                            | runtime (drain)                | 503 with `Retry-After` and `Connection: close`, an `err` frame, or UNAVAILABLE, answered directly (§10)                                                         |
| An `EventId` or `EventName` carrying a line terminator, an `EventId` carrying U+0000                                                                                                                         | runtime, where built           | `EventId::new` / `EventName::new` return `Err` (§3.1)                                                                                                           |
| Over a load-shedding limit                                                                                                                                                                                   | runtime                        | 503 with `Retry-After`, `REFUSED_STREAM`, close 1013, `unavailable`, or UNAVAILABLE                                                                             |
| Oversized frame or payload                                                                                                                                                                                   | runtime                        | 413, close 1009, or `bad_request` with `payload_too_large`                                                                                                      |
| Connect guard refusal                                                                                                                                                                                        | runtime                        | close code by kind or explicit `ConnectRefused::code`; a 401 or 403 before the upgrade under `refuse = handshake`                                               |
| Passed deadline, caller gone                                                                                                                                                                                 | runtime                        | cancellation with a `CancelReason`; `Timeout` rendering where a reply can still be sent                                                                         |

---

## 13. Decisions taken with defaults

1. **There is no per-module middleware.** One pre-dispatch stage in two sub-steps covers app-wide work scoped by path, and controller interceptors cover local work, both inside the error chain (§3.3). "Every controller in this module" takes the form of a module tier in the enhancer stack if a case needs it; until then `.at(prefix)` with a pattern-scoped entry covers it.
2. **`Conflict` maps to gRPC ABORTED.** Google's API guidance pairs HTTP 409 with both ABORTED and ALREADY_EXISTS. `CallError::grpc_code(..)` overrides it per error.
3. **`Timeout` renders as HTTP 504.** The server ran out of time, usually waiting on something downstream. 408 means the server gave up waiting for the _client's request_, which is a different situation.
4. **Connect guards run after the 101 handshake** and refuse with a close code, as [23] asks, since a close code is the only refusal a browser can read. `refuse = handshake` on a gateway refuses before the upgrade with an HTTP 401 or 403 instead, as RFC 6455 allows, for non-browser clients and proxy logs (§4.1).
5. **The WebSocket and RPC envelopes are this design's own**, with JSON by default and MessagePack or CBOR for binary. Nothing here aims at wire compatibility with NestJS or socket.io.
6. **A slow WebSocket consumer is disconnected** with 1008 by default, with drop-oldest as an option.
7. **gRPC reflection is on in debug builds and opt-in in release builds.** The GraphQL playground follows the same rule.
8. **`fw dev` socket holding is Unix-only** (§9).
