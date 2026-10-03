# Divergences: race 2a, HM (the HTTP attributes and the axum backend)

Every place `crates/ulo-http-macros` and `crates/ulo-http-axum` depart from `transports/DESIGN.md`,
from the spine, or fill what either leaves open. Each entry gives what the design or spine says,
what was written, and why. All await the user's sign-off.

## `ulo-http-axum`

### 1. Connections are served by hyper's builders, not by `axum::serve`

- **Spine:** `lib.rs` and `backend.rs` serve "through `axum::serve` with a fallback service",
  one `axum::serve` per listener, graceful on `draining`, aborted on `closing`; `listener.rs`
  implements `axum::serve::Listener` and runs the TLS handshake in `accept`.
- **Written:** one accept loop per listener, each a spawned task, and one task per connection in
  that loop's `JoinSet`. Each connection is served by `hyper_util::server::conn::auto::Builder`
  (TLS connections, and plain ones when h2c is on) or `hyper::server::conn::http1::Builder`
  (plain connections when h2c is off), with upgrades. Each request is mapped to
  `axum::extract::Request` and each response to `axum::response::Response`, as `axum::serve`
  maps them. The `axum::serve::Listener` impl is gone; the TLS handshake runs in the connection
  task.
- **Why:** `axum::serve` takes no configuration, and four settings or SPI duties depend on it:
  - it accepts the HTTP/2 preface on every plain connection, so `HttpConfig::h2c = false`, the
    default, could not hold;
  - it cannot set `SETTINGS_MAX_CONCURRENT_STREAMS`, so `HttpConfig::max_concurrent_streams`
    would be ignored;
  - it spawns connection tasks detached, so `Backend::close` could not close the connections
    left;
  - it awaits `Listener::accept` inline in its accept loop, so a TLS handshake there stalls
    every other connection behind a slow peer.
- **Consequence:** axum supplies only the edge types (`axum::body::Body` and the request and
  response aliases). Every behaviour is hyper's, as it is under `axum::serve`. Whether the crate
  keeps the axum name is the user's call.
- **Manifest:** `hyper-util` gains `server-auto`, `tokio` gains `time`; `tower` and
  `http-body-util`, now unused, are removed. No workspace dependency is added.

### 2. Plan point: the accept loop

- **Plan:** "One `axum::serve` per listener or one accept loop" is HM's to decide.
- **Written:** one loop per listener, each a spawned task, joined by `serve`. At the drain a loop
  drops its listener, which stops accepting, and waits for its connections to end; `close`
  aborts what is left through `JoinSet::shutdown`. Dropping the `serve` future aborts every loop
  and, through each loop's `JoinSet`, every connection.

### 3. Plan point: a failed TLS handshake, and a handshake timeout

- **Plan:** how a failed TLS handshake is logged is HM's to decide. The design names no handshake
  timeout.
- **Written:** a failed handshake is logged at `debug` with the peer address and the error, and
  the connection is dropped; the accept loop is unaffected, since the handshake runs in the
  connection task. A handshake is bounded by 30 seconds, logged at `debug` when it passes, and
  abandoned at the drain.
- **Why:** handshake failures are routine (scanners, clients refusing the certificate), and a
  higher level floods the log. 30 seconds is hyper's default HTTP/1.1 header-read timeout
  (entry 4), so a peer that connects and sends nothing is dropped after the same wait with TLS as
  without.

### 4. hyper's HTTP/1.1 header-read timeout is on

- **Design:** silent. `axum::serve` sets no timer, and hyper skips its default 30-second
  header-read timeout without one.
- **Written:** both HTTP/1.1 builders get `TokioTimer`, which turns on hyper's default: a
  request's head must arrive within 30 seconds of hyper starting to read it. hyper 1.11 starts
  that clock in `Conn::read_head`, which also runs while a keep-alive connection waits for its
  next request.
- **Why:** without it a peer holds a connection open indefinitely by sending a request head
  slowly, and the drain (entry 7) would wait on that connection until its deadline.

### 5. `max_concurrent_streams` unset keeps hyper's default

- **Design:** §3.6: `SETTINGS_MAX_CONCURRENT_STREAMS` bounds each HTTP/2 connection; `HttpConfig`
  holds it as `Option<u32>` with no stated meaning for `None`.
- **Written:** `None` leaves hyper's default, 200, which hyper documents as outside its stability
  guarantee; `Some(n)` sets `n`.
- **Why:** passing `None` to hyper removes the limit, which would leave a connection unbounded by
  default.

### 6. HTTP/2 extended CONNECT is not enabled

- **Design:** §3.5 hands a request carrying `Upgrade: websocket` to `ulo-ws`, an HTTP/1.1
  mechanism; WebSocket over HTTP/2 (RFC 8441) is not mentioned.
- **Written:** the HTTP/2 builder does not advertise `SETTINGS_ENABLE_CONNECT_PROTOCOL`.
  `axum::serve` does.
- **Why:** nothing in race 2a answers an extended CONNECT, and `ulo-ws` (race 2b) is designed
  around the HTTP/1.1 upgrade.

### 7. `drain` resolves when every connection has ended

- **Design:** §3.6, §10: at `drain` the backend stops accepting, sends GOAWAY, closes idle
  keep-alives and marks busy ones `Connection: close`. `Backend::drain` is documented as "Stops
  accepting".
- **Written:** `drain` raises the drain signal and resolves once `serve` has ended, which is once
  every connection's graceful shutdown has completed. GOAWAY, the idle close and the
  `Connection: close` header come from hyper's `graceful_shutdown` (hyper 1.11
  `Conn::enforce_version` inserts the header once keep-alive is disabled).
- **Why:** the core's drain window ends when every transport's drain future and every live
  execution have completed. A drain future that returned at once would let the window end while
  a response whose execution has finished is still being written, and `close` would then cut it.
  The window's deadline still bounds the wait.

### 8. Calls out of order

- **Design:** silent.
- **Written:** `serve` before `bind` answers `Err`. `drain` and `close` before `bind` do nothing.
  When `drain` or `close` arrives before `serve` has taken the listeners (an app shut down before
  it served, or `listen()` closing the servers it bound before a later bind failed), it closes
  them itself and marks the backend finished, and a later `serve` answers `Ok(())` at once.

### 9. `Request::upgrade` is `None` on a request that asks for no upgrade

- **Spine:** `Request::upgrade` is "`None` on a backend whose limits declare `upgrades: false`".
- **Written:** `Some` exactly when hyper stored an upgrade future on the request, which it does
  for an HTTP/1.1 request asking to upgrade; `None` on every other request, although the axum
  backend's limits declare `upgrades: true`.
- **Why:** hyper creates the future only for such a request, and a future for any other could
  only fail. A request is logged below.

### 10. Each connection's `local` address is the accepted socket's

- **Spine:** `convert::request(req, peer, local)` with both addresses optional.
- **Written:** `ConnInfo::peer` is always set, from `accept`; `ConnInfo::local` is the accepted
  socket's own address (the concrete interface address on a listener bound to `0.0.0.0`), unset
  only if the OS refuses to report it; `ConnInfo::tls` carries the negotiated ALPN protocol and
  the SNI name.

## `ulo-http-macros`

### 11. The pattern grammar follows the router's, empty segments included

- **Spine:** `pattern::check`: "a leading `/`; segments of static text, `{name}` with an
  identifier-like name, or a final `{*name}`; no brace elsewhere; no name twice".
- **Written:** the router's grammar as H wrote it in `router/pattern.rs`, with its reasons word
  for word: additionally no empty segment (`/a//b`); a name starts with `_` or a letter and
  continues with `_`, letters and digits (Unicode, as `char::is_alphabetic` reads it); one trailing
  slash dropped before the checks, so `/files/{*rest}/` passes.
- **Why:** a literal the macro accepts and the router refuses would fail only at `prepare`.

### 12. HTTP handlers declare no shape

- **Design:** X3 gives `HandlerSpec::shape`; §5 and §6 use it for RPC and gRPC; HTTP is silent.
- **Written:** the attribute passes no `.shape(..)`, so every HTTP handler, an SSE one included,
  is `Shape::Unary`.
- **Why:** telling a streaming return from another would mean reading its spelling (`Sse<..>`),
  which the design rules out for return types, and nothing in HTTP reads the shape.

### 13. The attribute's own refusals

- **Design:** §12: a malformed handler attribute is a macro span error.
- **Written:**
  - `#[get]` with no argument: ``#[get] takes the route pattern, as in #[get("/users/{id}")]``.
  - A method without the `__handler` attribute: ``#[get] goes on a method of a `#[routes]` impl,
    which hands it the handler's enhancers``.
  - An invalid literal: `invalid route pattern: {reason}`, spanned on the literal.
  - A generic handler, a wrong receiver and a non-async handler are `ulo-handler-codegen`'s to
    refuse or accept; the attribute adds no check of its own.

## Requests to other areas

- **H, `crates/ulo-http/src/request.rs`, `Request::upgrade`'s doc:** read "`None` when the request
  asks for no upgrade, or on a backend whose limits declare `upgrades: false`" (entry 9).
- **H, `crates/ulo-http/src/router/pattern.rs`:** a change to `parse_segments` or `check_name`
  needs the same change in `crates/ulo-http-macros/src/pattern.rs`; the two now accept the same
  set with the same reasons (entry 11).
- **P, `crates/ulo-http/src/backend.rs`, `Backend::drain`'s doc:** add that a backend may resolve
  once its connections have ended, which the core's drain window waits on (entry 7). Optional;
  the SPI's signature is unaffected.

No contract or signature mismatch with `ulo-handler-codegen` as MX filled it: `Paths::new`,
`protocol::take_handler_attr`, `params::analyze`, `reply::rewrite_opaque_returns`, `MountFn` and
`emit::call_ident` are called as they stand.
