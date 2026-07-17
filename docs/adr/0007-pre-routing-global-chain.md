# 0007 — The global middleware chain anchors before routing, per adapter, pinned by a conformance suite

Status: accepted

## Context

The global chain's documentation promised "runs before the adapter's routing on every request."
The implementations did not deliver that: all five HTTP adapters invoked `AdapterContext::execute`
*inside* each matched route's handler, plus once more in a path fallback for 404s. That anchor point
misses an entire request class — a known path with an unregistered method. The native router (axum's
`MethodRouter`, and each framework's equivalent) answers 405 before any toni code runs.

The gap is not cosmetic. CORS preflight is exactly the 405 shape: browsers send `OPTIONS /users` to a
route that only registers `GET`, so a `CorsMiddleware` on the global chain would never see the one
request it exists to answer. Request mutation had the same ceiling — rewriting the path inside a
matched handler is too late to change which route runs, so "pre-routing" middleware semantics
(Express/NestJS `app.use`) were unachievable.

The reference architecture here is NestJS's actual mechanism: Nest owns no router. It registers
routes onto the host framework and anchors global behavior at the host's own outermost point
(Express's middleware stack, Fastify's plugin/hook system). The guarantee is behavioral, realized
natively per adapter — and where the host can't express it, the adapter reaches for a host-native
substitute rather than a shared mechanism.

## Decision

The contract lives in core documentation; the mechanism lives in each adapter; conformance is a
test suite, not a hope.

**Contract** (on `AdapterContext::execute` and `HttpAdapter::into_lifecycle`): the global chain runs
once per inbound request, before route resolution. It observes every request — matched, unknown
path, method mismatch, WebSocket upgrade. It may short-circuit with a response. The request it
forwards is the one the router matches on.

**Conformance suite** (`integration-tests/tests/integration/global_chain_conformance.rs`): six
behaviors every HTTP adapter must exhibit — chain on matched route, on 404, on 405, middleware
answering preflight, short-circuit before the handler, and a path rewrite that changes the matched
route. The 405, preflight, and rewrite cases are the discriminators a post-routing anchor cannot
pass.

**Axum realization**: a `GlobalChainService` wraps the finished `Router` as a `tower::Service`; the
routing closure handed to `ctx.execute` is the entire router, not one handler. Both conversion
directions are re-wraps, not copies:

- *Requests*: `HttpRequest` wraps `http::Request<RequestBody>`, so native → toni → native preserves
  extensions (path params, hyper's upgrade slot) and body streaming.
- *Responses*: whatever the router produces — a toni handler's response, a native 405, a WebSocket
  handshake reply — is re-wrapped into `HttpResponse` for the chain to observe, body included.
  This rests on the Send-only response body model: `BoxBody` is `UnsyncBoxBody`, so axum's `!Sync`
  native body fits without buffering and streaming responses (SSE) flow through untouched.
  Response extensions do not survive the re-wrap; nothing relies on them (hyper drives WebSocket
  upgrades from the request side).

**Poem realization**: a `GlobalChainEndpoint` wraps the finished `Route` (itself an `Endpoint`).
One divergence from axum: poem's WebSocket upgrade slot lives in the `Request`'s internal state,
not in http extensions, so the original request shell is threaded through the chain in a take-once
slot and the chain's output (method, URI, headers, extensions, body) is written back onto it before
routing — rebuilding the request would drop the upgrade. Router errors (`MethodNotAllowedError`
et al.) convert via `into_response()`, which is how 405s reach the chain.

**Salvo realization**: salvo has no substitutable service type — `Server::serve` takes the
concrete `salvo::Service` — so the chain anchors as the goal of a catch-all router and drives an
inner `Service` through salvo's public `hyper_handler` entry, with the request shell threaded the
same way as poem's. Two host constraints shape the rest: `ReqBody::Boxed`'s inner type is
crate-private, so the chain's request body rides to the route handlers in a request extension
rather than being reconstituted into the salvo request; and salvo's router cannot distinguish a
method mismatch from an unmatched path (method and path are both opaque filters), so the fallback
consults the route table and answers 405 with an `Allow` header itself.

**Actix realization**: an App-level `Transform` middleware — routing happens inside the wrapped
service, so the middleware position is genuinely pre-routing. The distinctive constraint is `Send`:
the chain requires a `Send` routing closure, but actix's inner service is worker-local (`!Send`,
`Rc`-based). A oneshot channel bridge closes the gap — the routing closure sends the request to,
and awaits the response from, a worker-local dispatch future joined alongside the chain in the same
task. Mutations write back through `head_mut` plus a `match_info` refresh (the `NormalizePath`
precedent), which requires that no clone of the request exist during dispatch — actix's router
mutates `match_info` through `Rc::get_mut` while matching. The fallback carries the same
route-table 405 logic as salvo, since actix also falls through to `default_service` on method
mismatch.

**Rocket realization**: the internal-matching case. Fairings cannot short-circuit with a response,
so rocket offers no pre-routing anchor at all; instead one catch-all route per method hosts the
chain and routing is internal (`match_route` over the toni route table, including param capture and
the 405/`Allow` logic). Rocket's router reduces to connection serving. WebSocket upgrades need the
borrowed rocket request, which the `'static` routing closure cannot hold — the closure returns a
marker response for WS paths, and the outer handler performs the upgrade only if the marker
survived the chain, so middleware can reject upgrades by replacing the response.

## Considered and rejected

**A toni-owned router (`matchit`) in the serve path.** It would make the five native routers dumb
byte-pumps and re-implement path matching the hosts already do well. Nest never does this; the
adapter contract doesn't need it. `matchit` remains a legitimate private mechanism for an adapter
whose host offers no anchor point (rocket's fairings cannot short-circuit), and for a future
adapter-independent portless handler — both out of scope here.

## Consequences

- CORS, rate limiting, and auth on the global chain are now actually global on axum: 404s, 405s,
  and preflight all traverse the chain. A core `CorsMiddleware` becomes buildable.
- Middleware can rewrite the request pre-routing and change which route runs — Express/Nest
  `app.use` semantics.
- Same-port WebSocket handshake requests now traverse the chain (they bypassed it before). This is
  intended: the handshake is an HTTP request, and middleware can now reject upgrades.
- Conformance status: **all five adapters pass** — axum, poem, salvo, actix, rocket.
- The composed value — chain wrapped around router — is one `tower::Service`. Binding the socket is
  the only step after composition, which is the natural seam for portless dispatch (in-process
  testing, serverless) if that lands later.
- Per-request overhead on axum: one extra re-wrap in each direction — no copying, no buffering.
