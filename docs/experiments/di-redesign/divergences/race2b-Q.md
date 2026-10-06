# Divergences: race 2b, area Q (GraphQL)

Every place area Q departs from `transports/DESIGN.md` §7, the twentieth response or
`REVIEW_2B.md` R35–R39, or fills what they leave open. Each entry gives what the design says, what
was written, and why. Entries marked **sign-off** change a behaviour the design states; the rest
fill gaps.

No cargo was run. async-graphql 7.2.1 and juniper 0.16.2 were read from `~/.cargo/registry`; the
items a compile has to confirm are under "Not verified".

Files written: `crates/ulo-graphql-http/{Cargo.toml, src/lib.rs, src/controller.rs, src/module.rs,
src/playground.rs}`, `crates/ulo-graphql-ws/{Cargo.toml, src/lib.rs, src/gateway.rs}`,
`crates/ulo-graphql-async-graphql/{Cargo.toml, src/lib.rs}`, `crates/ulo-graphql-juniper/{Cargo.toml,
src/lib.rs}`. `crates/ulo-graphql` is unchanged: the spine left no `todo!()` in it.

## The public surface, as changed

Every signature the spine wrote is kept. Added:

```rust
// ulo-graphql-http
impl<Q> GraphqlConfig<Q> { pub fn connection_init_timeout(self, timeout: Bound) -> Self; }
impl<Q> Clone + PartialEq + Eq + Hash for GraphqlConfig<Q>;   // and for GraphqlModule<Q>

// ulo-graphql-ws
#[doc(hidden)] pub mod __private { pub struct InitTimeout(pub Bound); }
```

## Visibility

### 1. The engine is bound in a global module that exports it (sign-off)

- **Design:** §7's example binds `AsyncGraphql<ApiSchema, GqlContext> as dyn Engine` in `ApiModule`,
  which imports `GraphqlModule::for_root(..)`.
- **Written:** `GraphqlModule` registers the endpoint controller, and the gateway, as its own
  controllers, each reading `Dep<dyn Engine, Q>`. The crate docs and `GraphqlModule`'s doc write the
  example with `global` and `exports = [dyn Engine]` on `ApiModule`.
- **Why:** DESIGN §8.2 gives a module its own bindings, its direct imports' exports and the global
  modules' exports. The module importing `GraphqlModule` is none of those, so the example as written
  fails `wire()` with the engine missing from `GraphqlModule`. A controller cannot be registered into
  the importer, and `GraphqlModule` cannot import a module it does not know. The other spelling
  available without new core surface is the application listing the controllers itself, which needs
  `.at(path)` and so a hand-written module.

### 2. The context type is resolved with the engine module's visibility (sign-off)

- **Design:** §7 and R35: the adapter resolves `exec.get::<C>()` (juniper: `exec.get::<Q::Context>()`)
  per call.
- **Written:** each adapter reads `ModuleRef` at construction, its own module, and per call resolves
  `module.with_execution(&exec).get::<C>()`. The context is built once in the call's execution and
  cached there as `exec.get` would.
- **Why:** `exec.get` resolves with the execution's module, which is the dispatching controller's:
  `GraphqlModule` over HTTP and over the gateway. `GqlContext` is bound beside the engine, so
  `exec.get::<GqlContext>()` would miss it at every call unless it too were exported from a global
  module. The engine's module sees it.
- **Not checked at `wire()`:** neither adapter declares `Dep<C>` among its dependencies. Declaring it
  would make the engine, under `Auto`, per-execution through an execution-scoped context, and the
  controller with it. A context that is not bound fails each call (entry 4).

### 3. `dep` resolves with the execution's visibility, as designed

- **Design:** `ulo_graphql_async_graphql::dep::<T>(ctx)` reads `ctx.data::<ExecutionRef>()?.get::<T>()`;
  juniper's reads through the context's `ExecutionRef`.
- **Written:** as designed in both. The doc on each says the read has the execution's visibility,
  `GraphqlModule`'s, which entry 1 limits to the globals' exports, and that a binding only the
  engine's module sees is read through a field of the context, which is constructed in the context's
  own module.
- **Open:** async-graphql's `dep` could read through the engine module's `ModuleRef` instead (the
  adapter controls what it inserts as data); juniper's frozen signature takes `&impl
  AsRef<ExecutionRef>` and cannot. Both were kept on the design's reading so the one name means one
  thing.

## The engines

### 4. A context that fails to build answers a request error

- **Design:** silent.
- **Written:** the lookup error is logged at `error` and the call answers
  `GqlResponse::request_error(["the request's context could not be built"])`, the message withheld as
  `Internal` withholds one. Over HTTP under `application/graphql-response+json` that is 400.
- **Why:** `Engine::execute` answers a `GqlResponse` and has no error channel; nothing executed, and
  `Outcome` has two values. A third, `Outcome::Failed` rendered 500, would describe a server fault
  exactly; it changes `ulo-graphql`'s public enum and is left for the design.

### 5. async-graphql's context is inserted as `Dep<C>`

- **Design:** "inserts it with `Request::data`".
- **Written:** `request.data(context)` with `context: Dep<C>`, plus `request.data(exec)`. A resolver
  reads `ctx.data::<Dep<GqlContext>>()`.
- **Why:** the resolver hands back a shared `Dep`, and `C` is not `Clone`.

### 6. How an async-graphql response is told to be a request error

- **Design:** R39 adds `Outcome`; async-graphql does not report it.
- **Written:** `data` `null`, at least one error, and no error carrying a `path` reads as
  `RequestError`; anything else is `Executed`.
- **Why:** async-graphql answers a parse, validation or operation-selection failure with
  `Response::from_errors`, `data` `null` and no paths (`schema.rs` `execute`), and a resolver error
  always carries its field's path, a root field's included. A subscription sent over HTTP answers
  "Subscriptions are not supported on this transport." with no path, which therefore reads as a
  request error.

### 7. Juniper subscriptions without a task

- **Design:** `Engine::subscribe` on `resolve_into_stream` directly (Q15).
- **Written:** one future owns the root node, the context and the request, calls
  `juniper::http::resolve_into_stream`, and forwards each response through a bounded
  `futures::channel::mpsc` channel; the returned stream is `select(receiver, driver)`, so polling it
  drives the future and nothing is spawned. Each root field's stream yields `{field: value}` per
  event, an `Err` item as `{field: null}` with the error; several root fields are merged as they
  arrive. Errors `resolve_into_stream` returns beside the streams answer one executed response.
  `NotSubscription` runs the operation through `execute` and answers one response, so a query sent
  over graphql-transport-ws works.
- **Why:** `resolve_into_stream`'s stream borrows all three, so no `'static` stream can hold them
  apart from the future that owns them, and the adapter depends on no runtime. `juniper_subscriptions`,
  which waits for every root field before emitting, is not in the registry (Q15).

### 8. Juniper's root node is stored erased

- **Design:** `Juniper<Q, M, Sub>` reads `Dep<RootNode<'static, Q, M, Sub>>`.
- **Written:** a private `root: Arc<dyn Any + Send + Sync>` holding the `Arc<RootNode<..>>`, downcast
  per call. The downcast cannot fail; were it to, a call answers a request error and `sdl` an empty
  string.
- **Why:** a field naming `RootNode<'static, Q, M, Sub>` needs juniper's bounds on the struct
  definition, which would change the spine's public signature.

### 9. Juniper's `sdl` needs the `schema-language` feature

- **Written:** `juniper = { workspace = true, features = ["schema-language"] }` in the member.
- **Not in the offline registry:** `graphql-parser` and `void`, which that feature pulls; the first
  compile fetches them, as it fetches `ciborium` and `rmp-serde`.

### 10. Request extensions

- **Written:** async-graphql receives each extension converted to its value type; one that does not
  convert answers a request error. juniper reads no extensions and they are dropped.

## The HTTP endpoint

### 11. No `IntoReply<Http>` for `GqlResponse`; the controller builds the response (the spine's open point)

- **Design:** "the controller's `IntoReply<Http>` for `GqlResponse` applies [the rules]".
- **Written:** the controller is hand-written over `ulo_http::__private::HttpHandler` with two
  handlers, `GET /` and `POST /` under `.at(config.path)`, whose call closures build the
  `ulo_http::Response` directly: status from the negotiated media type and the `Outcome`, body the
  serialized `GqlResponse`. No public wrapper type is added.
- **Why:** the orphan rule refuses the impl in `ulo-graphql-http`. A wrapper would serve only a user
  writing their own GraphQL route, which nothing asks for. `HttpHandler` is the doc-hidden boundary
  generated and sibling code uses; `#[routes]` over a generic controller was the alternative, and the
  hand-written form has no macro behaviour to depend on.

### 12. An absent `Accept` reads as `application/graphql-response+json` (sign-off; the plan's open point)

- **Design:** §7: `application/graphql-response+json` "when accepted and `application/json`
  otherwise", which makes an absent `Accept` legacy JSON.
- **Written:** no `Accept` header, or only empty ones, negotiates `application/graphql-response+json`.
- **Why:** GraphQL-over-HTTP's watershed: from 2025-01-01 a server treats a request without `Accept`
  as accepting `application/graphql-response+json`, before it as `application/json`. The date has
  passed, and §0.3 lets the specification decide. The consequence: a client sending no `Accept`
  receives 400 for a document that does not parse.

### 13. `Accept` negotiation

- **Design:** "negotiated from `Accept`".
- **Written:** each media range's quality is read (RFC 9110 §12.4.2, three decimals); a type's
  quality is that of the most specific matching range (exact, `type/*`, `*/*`). The response is
  `application/graphql-response+json` when its quality is nonzero and at least that of
  `application/json`, so `*/*` and a tie choose it, and `application/json` otherwise, including when
  neither is acceptable: no 406 is sent. Both carry `; charset=utf-8`.

### 14. A GET mutation's 405 is built in the controller

- **Design:** "a GET whose operation is a mutation is refused with 405".
- **Written:** 405 with `Allow: GET, HEAD, POST, OPTIONS` (what the router computes for the path) and
  a request-error body in the negotiated media type saying to POST. It is not offered to the error
  handlers.
- **Why:** offering it as the router does needs a `MethodNotAllowed`, whose constructor is
  `pub(crate)` in `ulo-http` (request below).
- **How the operation is found:** `ulo-graphql-http` depends on no GraphQL parser. A scan of the
  document's top level finds each operation's type and name, skipping comments, strings and block
  strings and opening no selection set inside parentheses; the operation is the one named
  `operationName`, or the only one. When the scan cannot tell, the request executes and the engine
  reports whatever is wrong with it. `Engine` has no method answering an operation's type.

### 15. The other refusals

- **Written:** a POST whose `Content-Type` is not `application/json` returns
  `ExtractError::UnsupportedMediaType`, which the error handlers receive and which renders 415 with
  `Accept`. The body is read through `Bytes`' extractor, so a body over the route's limit is 413.
  A body that is not a GraphQL request, a missing `query` included, and `variables` or `extensions`
  query parameters that are not JSON objects, answer request errors.

### 16. The playground

- **Written:** served on a GET with no `query` when `GraphqlConfig::playground` holds and the client
  names `text/html` with a nonzero quality; a wildcard does not count, since `curl` sends `*/*`. The
  endpoint and subscription paths handed to `Engine::playground_html` carry `HttpCx::mount_prefix`, so
  the page works under an embedding host. `text/html; charset=utf-8`.

### 17. Paths

- **Written:** `GraphqlConfig`'s paths are normalized at registration to one leading `/` and no
  trailing one.

## Mounting and configuration

### 18. `GraphqlModule` mounts the graphql-transport-ws gateway (the spine's open point)

- **Design:** §7 lists `GraphqlConfig::at(..).subscriptions(path)` and the gateway in `ulo-graphql-ws`;
  §1 has `ulo-graphql-http` meet the core through `ulo-http`.
- **Written:** with `subscriptions(path)` set, `GraphqlModule::register` also registers
  `GraphqlWs<Q>` as a controller `.at(path)`. `ulo-graphql-http` depends on `ulo-graphql-ws`, and so
  on `ulo-ws`. The application still imports `WsModule` once; `GraphqlModule` does not import it.
- **Why:** the design's example imports `GraphqlModule` alone with `.subscriptions(..)`, and a gateway
  path set in the config with nothing mounted at it would be a setting that does nothing. Importing
  `WsModule::for_root()` from `GraphqlModule` would make a second `WsModule` identity beside an
  application's `WsModule::for_root().broadcast(..)`, two `Rooms` bindings and two hand-offs.

### 19. `connection_init_timeout` is set on `GraphqlConfig`, refused at `wire()` when zero

- **Design:** the gateway owns `connection_init_timeout: Bound`; §12 refuses
  `Bound::After(Duration::ZERO)` on it in `prepare`.
- **Written:** `GraphqlConfig::connection_init_timeout(Bound)`; 3 seconds at `Bound::Default`, the
  reference server's `connectionInitWaitTimeout`; `Unbounded` starts no clock. `GraphqlModule` binds
  it, module-private, as `ulo_graphql_ws::__private::InitTimeout`, which the gateway reads as
  `Option<Dep<InitTimeout>>`, so a gateway listed without `GraphqlModule` takes the default. A zero is
  bound through `ModuleDef::try_value` with X22's `zero_bound` text, and `wire()` reports it.
- **Why:** `GatewayConfig::settings()` is a static function, so the configured value reaches the
  gateway through the container, and nothing of `ulo-graphql-ws` runs in any server's `prepare`.

### 20. The qualifier, through a type parameter (the plan's open point)

- **Written:** `GraphqlConfig<Q>`, `GraphqlModule<Q>` and the private controller `GraphqlEndpoint<Q>`
  and `GraphqlWs<Q>` carry `Q`, and read `Dep<dyn Engine, Q>`; `Q = ()` is the unqualified key. The
  endpoint's path and playground setting travel as a module-private `EndpointSettings<Q>` value
  bound by `GraphqlModule`, so two `GraphqlModule`s, of one `Q` or two, each read their own.
- **Identity:** `ModuleIdentity::of_value(self)` over the configuration, which needed `Clone`,
  `PartialEq`, `Eq` and `Hash` on `GraphqlModule<Q>` and `GraphqlConfig<Q>`, written by hand so `Q`
  carries no bound.

## The graphql-transport-ws gateway

### 21. The protocol state lives in the session

- **Written:** `ConnectionInit`, the session type, holds the payload in a `OnceLock` (set once, by the
  first `connection_init`) and the connection's protocol state behind an `Arc`: whether it is
  acknowledged and the running operations by `id`. A clone shares the state. Both fields are
  `pub(crate)`; `payload()` keeps its signature.
- **Why:** the gateway instance is built by the container and may not be one per connection; the
  session is.

### 22. The connection's life

- **Written:**
  - `on_connect` refuses with 4406 "Subprotocol not acceptable" when `UpgradeHead::subprotocol()` is
    not `graphql-transport-ws`. A refused connection never reaches `on_disconnect`.
  - The init clock is a `tokio::spawn`ed sleep on the app's `Timer`; at expiry an unacknowledged
    connection closes 4408.
  - `connection_init` stores the payload and acknowledges under one lock, so a `subscribe` admitted
    after the ack finds the payload; a second closes 4429. `ping` answers `pong` echoing its payload;
    `pong` is accepted and unread. A binary frame, text that does not parse, an unknown `type` or a
    malformed `subscribe` closes 4400 "Invalid message received".
  - `subscribe` before the ack closes 4401; a running `id` closes 4409 with "Subscriber for {id}
    already exists", shortened to "Subscriber already exists" past 123 bytes. During the drain
    `open_execution` refuses, and the operation answers `error` "the server is shutting down".
  - Each operation runs in a `tokio::spawn`ed task holding its `Execution`. An `Executed` response is
    `next`, a `RequestError` is `error` with its errors and ends the operation without `complete`.
    The engine's stream ending writes `complete`; the drain beginning also writes `complete` and ends
    the operation; the client's `complete` cancels the execution with `ClientCancelled` and a
    disconnect with `Disconnected`, both ending it with nothing written. The stream is wrapped in
    `Tracked` and dropped after its end reached the wire.
  - An `id` may be reused once its operation has ended; a counter keeps an ending operation from
    removing a newer one under the same `id`.
- **Not in the design:** operations do not go through `dispatch`. The connect guards run once per
  connection; no `Ws` guard, interceptor or error handler runs per `subscribe`, the design's hand-
  written gateway having no handler for them to wrap.

## Requests for other areas

- **W (`crates/ulo-ws`, prefix join):** `GraphqlWs` declares `GatewaySettings::at("/")` and is
  mounted `.at(path)`. The request is that the gateway's served path be `HandlerInfo::prefix()`
  joined to `GatewaySettings::path` by `ulo_http`'s `Pattern::join` rule, one `/` between and a
  trailing one dropped, so `.at("/graphql/ws")` serves `/graphql/ws`. The design says `.at` applies
  "to every route and gateway path of that controller" without the join rule.
- **S (`crates/ulo-http/src/miss.rs`, `MethodNotAllowed::new`):** a public constructor, so a handler
  refusing a method, the GraphQL endpoint's GET mutation, can hand the 405 to the error handlers as
  the router does (entry 14).
- **Coordinator:** no root `Cargo.toml` change. The members enable `tokio`'s `rt`, juniper's
  `schema-language` and async-graphql's `graphiql`, and use the workspace's `futures`, `tracing`,
  `futures-util`, `serde` and `serde_json`.

## Replaced on `master`, and not carried over

- **Replaced:** `master`'s `ulo-graphql-async-graphql` (its controller, module, context builder and
  subscription gateway) and `ulo-graphql-juniper` (controller, module, service, factory, context
  builder, playground page). The spine removed both trees; nothing of them was read as spec.
- **Not carried:** the `GraphQLContextBuilder` trait, replaced by the context type parameter;
  async-graphql's own WebSocket protocol driver, since one gateway serves both engines; juniper's
  GraphQL Playground page beside GraphiQL; GraphQL batch requests, which neither design names and
  GraphQL-over-HTTP does not require.

## Not verified

- Nothing was compiled.
- juniper: `RootNode<'static, ..>` borrowed as `&'a RootNode<'a, ..>` by `GraphQLRequest::execute` and
  `resolve_into_stream` needs `RootNode` covariant in its lifetime, as `juniper_hyper` relies on;
  the futures of both being `Send` under the adapter's bounds; `RootNode` being `Send + Sync` for
  `Dep`.
- serde: the internally tagged `ClientMessage` with the empty struct variant `Pong {}` accepting a
  `payload` key.
- The `select(receiver, once(driver).filter_map(..))` item type inferring as `GqlResponse`.
- The ranked `Cancelled`, `Draining` and W's `Connection::send` futures being `Send`, which
  `tokio::spawn` of an operation requires.
- `GraphqlWs` reaching the wire depends on W's `Gateway::mount`, `Connection` and hand-off bodies,
  written in parallel.
