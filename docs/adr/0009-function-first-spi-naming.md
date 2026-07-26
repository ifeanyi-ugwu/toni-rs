# 0009 — Adapter SPI names: function first; a shared name requires shared semantics

Status: accepted

## Context

The adapter SPI had settled on `bind` as its registration verb: `HttpAdapter::bind` registered a
route, `HttpAdapter::bind_ws` an upgrade path, `RpcAdapter::bind` patterns plus the dispatch
callbacks, `WebSocketAdapter::bind` a gateway path, and `GrpcAdapter::bind` service registrations —
plus, alone among the five, actual socket acquisition. None of the first four touch a socket:
listener transports acquire theirs in `into_lifecycle`, and brokered transports (NATS, Redis,
RabbitMQ, MQTT, Kafka) never bind a listener at all — they establish broker connections, some
lazily inside the serve future.

In Rust, `bind` is not a neutral word. `TcpListener::bind`, `UdpSocket::bind`, and every server
framework's bind call acquire a socket. An SPI that uses the word for registration misleads on
first contact — and `ToniApplication::bind`, which does acquire sockets, used the same word one
level up with the standard meaning.

This surface has a history of renames, several driven by symmetry rather than by what the method
does at its site:

- `on_upgrade` → `bind_gateway` (`c437336`)
- `bind_gateway` → `create`/`attach`/`listen` (`9ce4487`)
- `listen` → `create` (`db718c9`), then `create` → `listen`/`serve` (`0daf9dd`)
- `listen` + `close` → `into_lifecycle` (`d364547`, `22076ef`)
- `add_route` and the `port()`/`hostname()` accessors dropped when `HttpAdapter` was aligned with
  the `WebSocketAdapter`/`RpcAdapter` contract (`5f29670`)

The pattern in that churn: a name chosen for one trait was propagated to the others for
consistency, then renamed again when it fit some site poorly. Consistency was treated as a goal.
It is a constraint — one that only applies among names that are each already accurate.

## Decision

A method is named for what it does at its own site. A name is shared across traits only when the
semantics are shared. When no single name truthfully describes every site, the names diverge;
symmetry never outranks accuracy.

Applied to the SPI:

- **Registration methods say what they register**: `register_route` / `register_ws_route` (HTTP),
  `register_handlers` (RPC), `register_gateway` (WebSocket), `register_services` (gRPC). The
  shared `register_` prefix marks the genuinely shared part — configuration accumulated before any
  I/O — and the suffixes differ because the registered objects differ.
- **`into_lifecycle` keeps its name.** The contract it states — consume the configured adapter,
  return a self-contained lifecycle handle — is the one thing uniform across every transport. An
  effect verb (`bind`, `connect`, `start`) would misname part of the fleet: listener transports
  bind synchronously there, brokered transports connect to their broker instead, and the
  orchestrator never branches on which.
- **`bind` means socket acquisition, nothing else.** `ToniApplication::bind` keeps it. gRPC's
  socket acquisition moved out of registration into `into_lifecycle`, making `register_services`
  registration-only like its siblings; port-in-use still surfaces as `Err` from `app.bind()`,
  which awaits `into_lifecycle`.

## Consequences

- Breaking for adapter implementors, invisible to applications. All implementations live
  in-workspace.
- `bind` now has one meaning in the workspace: a grep returns socket acquisition and the app-level
  lifecycle method, nothing else.
- Future SPI naming starts from the site: name what the method does; adopt an existing family name
  only if it is also true at the new site. A shared name that needs a doc comment to explain why
  it does not mean what it usually means is the wrong name.
