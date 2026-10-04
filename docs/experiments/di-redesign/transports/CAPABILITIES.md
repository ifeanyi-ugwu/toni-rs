Design the transport layer of a Rust application framework, on top of the dependency-injection core you already designed (the attached DESIGN.md). That core is settled and built; it compiles. Below is what the transport layer must be able to do and the constraints it lives under. Nothing here describes an existing implementation; design it from these requirements.

**The foundation.** Everything sits on the DI core's transport SPI: the `Transport` marker and its cheap-clone `Send + Sync` per-call context, `Server` (bind, serve, drain, close) and `DrainToken`, `Controller`/`Mount` and the hidden `__handler` protocol a transport's handler attributes consume, `EnhancerSpec` and the `dispatch` pipeline (guards, then interceptors, then the handler; errors through the error handlers; panics as `PanicRecovered`), execution inputs declared with `input::<T>().seeded_by::<T>()`, executions with cancellation, a draining notice and a deadline, and the app's `Timer`. Extend that SPI where a requirement needs it, and say where.

**Constraints**
- Stable Rust 1.88, edition 2024; nothing nightly.
- The DI core depends on no async runtime. A transport crate may, and a runtime crate supplies the `Timer`, the OS signal future and spawn helpers.
- Every macro is sugar over a public value-level API an integration can call without macros.
- Shared instances and contexts are `Send + Sync`; the application is `Send`.
- Wire behaviour follows the governing specification wherever one exists (HTTP semantics, Server-Sent Events, RFC 6455 WebSocket, gRPC, MQTT v5, AMQP and so on); where a specification decides, the design does not invent.

**Capabilities**

Common to every transport
1. Handlers are methods on a controller, marked by a transport's own attribute. One controller can serve several transports, and each handler checks its enhancers against its own transport's roles.
2. A handler's parameters are typed extractors read from the call: route parameters, query, body or payload, headers or metadata, the context itself, extension-bag values, execution inputs. At most one parameter consumes the body, checked at compile time naming both parameters.
3. Extraction can validate (declarative field rules), and an extraction failure has one documented shape per transport that the error handlers can reshape.
4. A handler returns a typed answer, converted per transport: nothing, one value, or a stream; fallible or not, the error side always reaching the error handlers whatever the return type is spelled as (an alias of `Result` included).
5. One error model across transports: a domain error declares a transport-neutral kind (bad request, unauthorized, forbidden, not found, conflict, unprocessable, too many requests, timeout, unavailable, unimplemented, internal) with a message and optional structured details, and each transport renders it canonically.
6. Declared metadata on a handler or its controller (roles required, rate classes), readable by guards and interceptors; the more specific declaration wins, and all declarations are listable.
7. Cancellation reaches the handler when the caller goes away: a dropped streaming response, a closed connection, a cancel frame, a passed deadline. A stream that ends cleanly is distinguishable from one cut off.
8. Graceful shutdown per the core's sequence: stop accepting, give each protocol its own close signal (GOAWAY, closing idle keep-alives, WebSocket 1001), drain in-flight calls, then close.
9. Binding to a host and port, to port 0 with the bound address reported, or to a pre-bound listener handed in by a supervisor (socket activation). A configuration error reports before a port conflict, and a failed bind leaves nothing half-bound.
10. TLS wherever the protocol supports it, a bad certificate failing startup rather than the serve loop.
11. Load shedding: a bound on in-flight calls per server and per connection where it applies, with a protocol-correct refusal.
12. Each transport declares its key (such as `http`), so a transport-scoped enhancer entry on a controller is checked against real keys and a misspelling is a compile error.
13. A controller-level enhancer declared by value is built once and shared by every handler of that controller.
14. A tracing span per call, carrying the transport and the route or pattern.

HTTP
15. Routing by method (GET, POST, PUT, PATCH, DELETE, HEAD, OPTIONS) and path, with `{name}` parameters, trailing slashes insignificant, and a duplicate route refused at startup. A miss is 404 and a wrong method 405 with `Allow`.
16. Backends and embedding: `fw-http-hyper` is the reference backend and the default, owning the listener and serving connections. One application runs unchanged inside an existing axum, salvo, poem, actix-web or rocket application, each adapter a separate crate, nested under a path or as the host's fallback, with the host's own middleware around it and the host keeping its own server setup; routing, extraction, pre-dispatch, dispatch and error rendering run inside the app either way. One conformance suite runs every adapter against the hyper reference, with documented limits where a host cannot do something.
17. Extractors for path, query, JSON, form, raw bytes, a streaming body and multipart, with a configurable body size limit.
18. Responses with any status, headers and body: JSON, bytes, a streaming body, and Server-Sent Events (events with data, id, event and retry, comment-only keepalives, and the Last-Event-ID a reconnecting client sends).
19. A global middleware chain that runs before routing (it sees misses, can rewrite the path, and can answer without reaching a route, for CORS preflight or authentication), and per-module middleware after routing, selected by route pattern with exclusions.
20. A built-in CORS middleware, and composition with tower layers.
21. WebSocket upgrades on the HTTP server's port, or on a separate port.

WebSocket
22. Gateways at a path, optionally namespaced; message handlers dispatched by an event field in each frame, the field name configurable (graphql-ws uses `type`).
23. Connect guards that refuse the upgrade with a protocol-correct close code (policy, rate limit, server fault, or a protocol-specific refusal code).
24. Per-connection session state, created before connect guards run and dropped with the connection; per-message state lives in the execution.
25. Connection hooks: on connect (able to refuse), on disconnect (knowing why), after the gateway initialises.
26. A message handler answers with nothing, one message, or a stream of messages; control frames are answered by the protocol, not by handlers; every failure answers one canonical error envelope.
27. Rooms and broadcast: to everyone, a room, a client, everyone except some; rooms joined and left at runtime; a multi-process mode over Redis with the same API.

RPC (message patterns)
28. Request-reply and fire-and-forget event patterns, matched by a pattern string, with per-call headers.
29. All four call shapes: one request and one reply, a streamed reply, a streamed request, and both streaming; a caller can cancel mid-stream.
30. Pluggable transports with one wire grammar: TCP, UDP, NATS, Redis, RabbitMQ, MQTT v5 and Kafka, each a separate crate, connecting lazily, with documented per-transport limits (datagram size, ordering).
31. A client that calls remote patterns (request, emit, stream) with a timeout, injectable like any binding.
32. A pattern nothing handles is reported the same way on every transport, and what a caller sees for a given failure is uniform across transports, a shared conformance suite proving it.
33. Binary payloads where the transport can carry them, refused clearly where it cannot.

gRPC
34. Services generated from `.proto` files by a build step that needs no system protoc; all four call shapes; handler parameters for the message, the inbound stream, the whole request and the context.
35. Reply metadata, the caller's deadline from `grpc-timeout` mapped to cancellation, error kinds mapped to canonical status codes, and structured error details sent in `grpc-status-details-bin`.
36. Reflection and health services, TLS, and generated clients registered and injected as ordinary bindings.

GraphQL
37. A GraphQL endpoint over HTTP and subscriptions over WebSocket (graphql-transport-ws), for at least two engines (async-graphql and juniper), with a playground in debug builds and the schema's context built per execution.

Runtime and tooling
38. A runtime crate supplying the core's `Timer`, an OS signal future (SIGINT and SIGTERM) for `serve`, and spawn helpers.
39. A development command that watches the source, rebuilds and restarts the app, and can hold a listening socket across restarts so clients never see a refused connection.

**What to deliver**
- The crate layout of the transport layer and how each crate meets the core's SPI, naming every extension the SPI needs.
- Per transport: the user-facing API with a short example (a controller with handlers, extraction, an answer, an enhancer), the context type, the wire behaviour on success and failure, and graceful shutdown.
- How a backend or broker crate plugs into its transport, written without macros.
- What is refused, and where: compile time, startup or runtime.
