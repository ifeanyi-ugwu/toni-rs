# 0011 — Trailing slashes are insignificant: canonicalized at registration and at the chain entry

Status: accepted

## Context

Route paths reached the native routers exactly as `join_route` composed them, and every native
router matches strictly: a controller at `/app` with `#[get("/")]` answered `/app` while `/app/`
404'd. The reverse hole existed too — `#[get("/x/")]` registered `/app/x/`, unreachable under the
slash-less form. Which requests worked depended on whether the client happened to append a slash,
and the answer differed from the Express/NestJS behavior the framework otherwise mirrors
(non-strict routing: `/app` and `/app/` address the same route).

The fix cannot live in the adapters. Five routers have five strictness models, and axum, actix,
and poem each solve this with their own normalization middleware — per-adapter wiring for one
semantic. The framework already owns a single pre-routing point on every adapter:
`AdapterContext::execute` (ADR-0007), through which every inbound request passes before the global
chain and route resolution.

## Decision

Trailing slashes are insignificant on the HTTP surface. Canonical form: no trailing slash, root
`/` excepted. Both sides normalize:

- **Registration**: `join_route` trims trailing slashes from the joined path, so `#[get("/x/")]`
  and `#[get("/x")]` register the same route. Same-port WebSocket upgrade paths and
  `RoutePattern` paths (module-middleware `for_route`) are trimmed the same way.
- **Request**: `AdapterContext::execute` trims trailing slashes from the request path — query
  string preserved — before the global chain runs. Trimming *before* the chain, not between chain
  and routing, keeps middleware path checks and route matching consistent: an auth middleware
  guarding `/admin` cannot be sidestepped by requesting `/admin/`. A middleware URI rewrite is
  forwarded to routing verbatim; rewrites are expected to target canonical paths.

The request is rewritten, not redirected. A redirect (Django's `APPEND_SLASH`, axum 0.5's router
redirect) costs a round-trip, breaks non-idempotent requests unless 307/308 is chosen carefully,
and is observable by clients; a rewrite is the Express behavior and free.

Conformance suite (`integration-tests/tests/integration/trailing_slash_conformance.rs`): per
adapter — controller root under both forms, parameterized paths under both forms, query survival,
slash-declared routes reachable, root `/` untouched.

## Consequences

- `/app` and `/app/` (and `/app///`) are the same route on all five HTTP adapters, including
  same-port WebSocket upgrade paths. Separate-port WebSocket (tungstenite) keeps verbatim
  matching — its handshake never passes through `AdapterContext`.
- Handlers, extractors, and middleware observe the canonical path; the slashed form a client sent
  is not recoverable downstream.
- Two routes distinguished only by trailing slash can no longer coexist — they collapse to one
  registration, surfacing as a duplicate-route error from the native router at bind.
- No opt-out. Strict-slash routing would need a knob threaded through `join_route` and
  `AdapterContext`; no use case has asked for it.
