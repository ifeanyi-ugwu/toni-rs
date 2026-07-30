# 0013 — Where to listen is a value: `BindTarget` carries an address or an existing listener

Status: accepted

## Context

An HTTP adapter could only be told an address to bind. `use_http_adapter(adapter, port, hostname)`
stored the pair, and `bind()` handed it to `HttpAdapter::into_lifecycle(port, hostname, ctx)`, which
bound its own socket. Three callers want the inverse — to acquire the socket first and hand it over:

- A watch-restart development loop. The rebuild takes seconds, and the process is gone for all of
  them. If the listening socket outlives the process, connections queue in the kernel accept backlog
  instead of being refused, and the restart shows up as latency rather than as errors.
- systemd socket activation, and zero-downtime restarts generally: the supervisor holds the socket
  across process replacements.
- Test harnesses. Binding port 0 and reading the assigned address back is a two-step dance today;
  binding the socket in the test removes the window where another process can take the port.

All three are the same request, and none can be satisfied from outside the framework. The
convention for passing a socket into a process is settled (`LISTEN_FDS`, systemd's protocol, which
the `listenfd` crate reads), and supervisors that speak it already exist. What was missing is a
parameter of listener type anywhere in toni's API: an inherited socket could reach the process but
had nowhere to go.

Addressing is not expressed one way across the framework, which decides how far the change reaches:

- **HTTP** — core owns the address. It is stored on `ToniApplication` and passed to the adapter.
- **RPC and gRPC** — the adapter owns the address, as a constructor argument (`TcpAdapter::new(host,
  port)`); `into_lifecycle()` takes none.
- **Separate-port WebSocket** — the gateway declaration owns the address (`#[websocket_gateway(port
  = N)]`), collected per port inside `bind()`.

## Decision

Where to listen becomes a value, `BindTarget`, with two forms: an address to bind, or a
`std::net::TcpListener` that is already bound and listening. It replaces the `(port, hostname)` pair
in `use_http_adapter` and in `HttpAdapter::into_lifecycle`, and both are reached through
`impl Into<BindTarget>`, so `("127.0.0.1", 3000)` and a listener are equally direct.

`std::net::TcpListener` is the currency rather than a runtime-specific type: it is what fd
inheritance produces, and it keeps the SPI free of a runtime commitment. Adapters convert on
adoption — `set_nonblocking(true)` then `from_std`, already the idiom in toni-grpc, toni-tcp, and
toni-udp. `BindTarget::into_std_listener` collapses both arms to one listener, binding only for the
address form, which is the whole of the change for an adapter that already serves on a constructed
listener.

The change is scoped to HTTP, because that is where core owns the address. RPC and gRPC adapters own
theirs, so a listener reaches them through a constructor, with no SPI involvement — added per crate
when wanted. Separate-port WebSocket is left out: an inherited socket carries no indication of which
gateway's declared port it is meant to satisfy, and port numbers there encode intent (a gateway
declaring `port = 0` wants its own listener, not a shared one), so the mapping is an open question
rather than an omission.

Acquisition stays outside the framework. Core accepts a listener and asks nothing about its origin;
it never reads `LISTEN_FDS` or any other protocol. Because the protocol is the standard one,
supervisors are interchangeable, and one seam serves the dev loop, socket activation, and test
harnesses alike.

An adapter that cannot serve on an existing listener returns an error naming the limitation, the
same capability-honesty pattern as `register_ws_route`'s default. Rocket is that adapter: it binds
inside `launch()` from figment configuration, with no public hook for a listener.

Conformance suite (`integration-tests/tests/integration/bind_target_conformance.rs`): per adapter, a
listener bound by the test is served on, proven by address identity — the application must report and
answer on the address recorded before handover, which a fresh bind on port 0 would not match. Rocket
is pinned to refuse at `bind()`.

## Consequences

- `use_http_adapter(adapter, port, hostname)` becomes `use_http_adapter(adapter, (hostname, port))`,
  and `HttpAdapter::into_lifecycle` takes a `BindTarget`. Both are breaking; every implementation and
  call site is in-tree.
- Port conflicts still surface at `app.bind()`. Adopting a listener happens at the same point, so a
  dead inherited socket surfaces there too, as `BindError::Setup`.
- `BoundAdapters` and port-0 resolution are untouched: addresses are read from the socket either way.
- Serving on an inherited socket is available on axum, poem, salvo, and actix, and unavailable on
  rocket — a capability difference the matrix records, in the company of same-port WebSocket.
- `BindTarget` is `#[non_exhaustive]`, leaving room for a Unix-socket or listener-set form without a
  further break.
