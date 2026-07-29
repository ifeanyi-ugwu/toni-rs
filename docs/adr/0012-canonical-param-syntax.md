# 0012 — `{param}` is the canonical route parameter syntax; `:param` is a compile error

Status: accepted

## Context

Two parameter syntaxes floated through the framework. Routes were originally declared Express-style
(`/users/:id` — the "toni-form" the adapter internals translate from), while the documentation and
newer code write OpenAPI-style `/users/{id}`. Which one worked depended on the adapter: `:id` was
translated for axum and salvo, native on poem, and matched internally on rocket and actix; `{id}`
was native on axum, actix, and salvo but a dead literal on poem and rocket, and invisible to the
salvo/actix 405 fallbacks. Five adapters times two syntaxes is ten translation points to keep in
sync, and the holes above are what that produced.

The choice of canonical form is not close. `{param}` is the URI-template (RFC 6570) and OpenAPI
convention, the native syntax of Go 1.22, Spring, and ASP.NET, and where the Rust ecosystem moved —
axum 0.8 deliberately migrated from `:param` to `{param}` because `:` is a legal literal path
character (`/users/123:activate` is a real URL shape), which makes a leading colon ambiguous,
while raw `{` `}` are illegal in URIs and therefore unambiguous delimiters.

## Decision

`{param}` is the parameter syntax. Declaring a route segment with a leading `:` is a compile
error with a migration hint (`route path segment ':id' uses ':param' syntax; toni's parameter
syntax is '{id}'`), emitted where the macros parse the path literal — the controller prefix and
the verb/`#[sse]` sub-paths. Mid-segment colons stay legal; only a segment-leading colon is
rejected.

Rejecting rather than translating follows the established precedent for retired declaration forms
(inline-struct controllers, ADR-0004): one form, and a loud error that names the replacement.
Supporting both silently would keep all ten adapter translation points alive forever.

The adapter SPI (`register_route`) keeps its existing leniency: paths are documented as `{param}`,
but the per-adapter translation and matching layers continue to accept `:param` — callers of the
raw SPI bypass the macros, and removing working leniency there buys nothing.

Conformance suite (`integration-tests/tests/integration/param_syntax_conformance.rs`): per
adapter — a `{param}` route extracts its parameter, and a method mismatch on a parameterized path
answers 405, which pins the route-table fallbacks (salvo, actix, rocket) that must recognize
`{param}` segments.

## Consequences

- `:param` routes fail to compile with a pointed error; migration is mechanical.
- The `Path<T>` extractor and route metadata are unaffected — parameter names never carried the
  sigil.
- Literal-colon segments (`/users/{id}:activate`-style custom methods) remain declarable.
- Adapter-internal translation from `:param` (axum, salvo) and matching of it (rocket, actix,
  salvo fallbacks) is now legacy surface reachable only through the raw SPI.
