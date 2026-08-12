# 0014 — A separate-port WebSocket listener is keyed by the port the gateway declares

Status: accepted

## Context

[0013](0013-bind-target-listener-adoption.md) made where-to-listen a value and threaded it through
the HTTP adapter, and left separate-port WebSocket out. The reason it gave: an inherited socket
carries no indication of which gateway's declared port it is meant to satisfy, and port numbers
there encode intent rather than a location — a gateway declaring `port = 0` wants a listener of its
own, not a shared one. So the mapping from socket to gateway was the open question.

Addressing for these gateways is owned by the declaration, `#[websocket_gateway("/p", port = N)]`,
and nothing else knows about `N` until `bind()` collects it. What `bind()` collects is already a
list of unique declared ports, and each one is the key the adapter filed its callbacks under in
`register_gateway`. The second element of that pair was a hostname string, present only so the
adapter could build an address to bind.

## Decision

The mapping is stated by the caller, keyed on the number already written in the gateway attribute:

```rust
app.use_websocket_listener(4001, listener)?;   // for #[websocket_gateway("/p", port = 4001)]
```

`WebSocketAdapter::into_lifecycle_handles` takes `Vec<(u16, BindTarget)>` in place of
`Vec<(u16, String)>`. The `u16` is the declared port and nothing more: it selects a gateway, and the
adapter must not treat it as a location. Where to listen is the `BindTarget`, which absorbs the
hostname that the string used to carry. The two can disagree — a socket bound to 4100 can serve the
gateway that declares 4001 — and `WsLifecycleHandle::local_addr` reports what the socket says, so
`BoundAdapters::websocket` stays truthful either way.

Nothing is inferred from the socket. `LISTEN_FDNAMES`, or fd ordering, would put half the mapping in
the supervisor's configuration where the application source cannot show it; keying on the declared
port keeps both halves visible at once — the attribute names the port, the call names the same port.

Rejected: a per-adapter constructor, the shape 0013 chose for RPC and gRPC. Those adapters own their
address, so a listener belongs in their constructor. A WebSocket adapter does not own its address —
it is handed one per declared port at `bind()` — so an adapter-side listener would have to be
reconciled against that list anyway, in each of the four implementations.

A supplied socket matching no gateway is logged as an error and left unserved rather than failing
`bind()`, which is how every other gateway-wiring mistake is reported. Naming the port is what
matters: the symptom otherwise is a port that accepts nothing and says nothing.

## Consequences

- Breaking for `WebSocketAdapter` implementors. Four are in-tree (axum, poem, salvo, tungstenite);
  each already resolves a `BindTarget` on its HTTP path, so adoption is the same three lines.
- Actix and rocket implement no `WebSocketAdapter` and are unaffected. Rocket's inability to adopt a
  listener is an HTTP-side limit and does not extend here.
- A gateway declaring `port = 0` can be handed a listener: the key is 0, and the socket's own port
  is what gets reported. Two gateways declaring 0 still collapse to one listener, as before.
- The declared port becomes a key rather than a reservation, so tests that hand over port-0 sockets
  can share one declared port and still run concurrently.
- Every socket a toni process listens on can now be inherited: HTTP through
  [0013](0013-bind-target-listener-adoption.md), RPC and gRPC through their constructors, and
  separate-port WebSocket here.
