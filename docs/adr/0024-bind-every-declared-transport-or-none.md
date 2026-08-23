# 0024 — An application binds every transport it declares, or none

Status: accepted

## Context

`ToniApplication::bind` wires four transports: same-port and separate-port WebSocket, RPC, gRPC, and
HTTP. Only the HTTP path propagated failure. Every other adapter's registration and bind errors were
logged at `error` level and execution continued, as were four configuration errors — a same-port
gateway with no HTTP adapter, a separate-port gateway with no WebSocket adapter, RPC controllers with
no RPC adapter, and a socket handed to `use_websocket_listener` for a port no gateway declares.

An application whose RPC adapter could not take its port returned `Ok` from `bind()` and went on to
serve HTTP.

### The return value cannot report a partial start

`BoundAdapters.rpc` is an `Option<SocketAddr>`, and `None` is the correct value for a subject-based
transport — NATS, Redis, MQTT, Kafka, RabbitMQ — that has no local listener to report. A caller
inspecting the result cannot separate *bound, with no address* from *never came up*. The same holds
for `websocket`, where an empty vector is the honest answer for an application with no separate-port
gateways.

So the failure was reachable only by reading logs. Nothing in the type could carry it, which is why
adding a field per transport would not have helped either.

### A half-started process reads healthy

The observable symptoms of a swallowed bind failure are the ones that mislead most: the process is
alive, its HTTP readiness probe answers 200, its exit code is unset, and the transport that is
missing fails at its callers as a connection refusal that looks like a network problem. A supervisor
configured to restart on failure has nothing to restart on.

## Decision

### A transport that cannot start fails the bind

An adapter that will not take its handlers, or cannot acquire its socket, returns
`BindError::Adapter { transport, source }`. The `transport` field names one of `http`, `websocket`,
`rpc`, `grpc`, so a caller can report which half of a multi-transport application is unavailable
without parsing a message.

### A declaration with nothing to serve it fails the bind

The four configuration errors return `BindError::Setup`, naming the call that is missing. Each
describes an application asking for something it never wired: patterns that no transport carries, a
gateway on a port nothing listens to, a socket nothing will accept on. The distinction from
`Adapter` is worth keeping — one is an environment that refused, the other is an application that is
incompletely assembled — but neither is a state worth starting in.

### Sockets acquired before the failure are closed

The transports bind in order, so a failure can find earlier ones already listening. Every adapter
that came up is closed and its handle dropped before the error returns. Closing first rather than
only dropping gives each adapter its shutdown path; dropping is what releases the socket, which would
otherwise stay open for as long as the caller holds the application.

### A failed bind is terminal

`bind()` consumes the adapters it wires, so a second call would find them gone and report the wrong
thing — an application with a port conflict would come back as one with no adapters configured.
`AppState::Failed` makes the second call say what is true instead.

### `BindError` is `#[non_exhaustive]`

A fifth transport, or a finer split of the existing variants, should not be a breaking change for
everyone matching on the enum.

## Consequences

- A port conflict on any transport surfaces at `app.bind()`, with the same shape it already had on
  HTTP.
- An application that compiles in more transports than it wires fails at startup rather than serving
  the subset it managed. Deployments that vary by transport choose their module set or their
  adapters per binary, which is a decision made where it can be seen.
- Downstream code matching on `BindError` needs a wildcard arm.
- Adapter authors get a stronger contract to write against: an error returned from `register_*` or
  `into_lifecycle` stops the application rather than degrading it.

## Roads not taken

**Log and continue for the configuration errors, propagating only real bind failures.** A binary
that compiles in RPC controllers and deploys HTTP-only is a real shape. It is served by selecting
the modules that binary builds, though, not by declaring a transport and relying on nobody noticing
its absence. Under the lenient reading the common case — a `use_rpc_adapter` call that was never
written — stays silent, and it is far more frequent than the deliberate one.

**Reporting failures through `BoundAdapters` and leaving `bind()` infallible.** It puts the decision
in the one place callers reliably skip. `bind().unwrap()` appears in every example in the repository
and would ignore the field entirely.

**Draining with a timeout on the teardown path.** Nothing has been served at that point, so there is
no in-flight work for a drain deadline to protect.
