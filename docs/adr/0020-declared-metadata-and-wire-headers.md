# 0020 — Declared metadata reaches every transport, and wire fields are headers

Status: accepted

## Context

`#[set_metadata(...)]` annotates a handler with configuration a guard reads back — the mechanism behind
a roles guard, a rate-limit tier, a feature flag on one route. `RouteMetadata` carries it, every context
exposes it through `HandlerContext::route_metadata`, and the plumbing is complete end to end.

Almost nothing fills it.

### One macro reads the attribute, and only from methods

`set_metadata` is consumed by a single file, `controller_macro/instance_injection.rs`, and it reads
`&method.attrs`. So:

- The WebSocket, RPC and gRPC macros do not handle the attribute at all. Their trait defaults return an
  empty map that no generated code overrides.
- Controller-level `#[set_metadata]` is collected nowhere, HTTP included. The class half does not exist
  on any transport.

Against Nest's `getAllAndOverride([getHandler(), getClass()])`, toni has the handler half on one
transport out of four.

### An empty map does not fail; it authorises

A universal guard compiles today:

```rust
impl<C: HandlerContext> Guard<C> for RolesGuard { … }
```

because `route_metadata()` and `extensions()` are both on `HandlerContext`. Registered on a WebSocket
gateway it does not error and does not refuse — it reads an empty map, finds no requirement, and waves
every message through. It fails open, and nothing reports it.

Population is the whole fix. Once the attribute is collected, "empty" honestly means "not annotated",
which is the semantic Nest already relies on (`if (!required) return true`).

### Two things are called metadata, on one object

`ctx.metadata()` on `RpcContext` and `GrpcContext` returns the *wire* fields — NATS headers, AMQP
headers, Kafka record headers, MQTT user properties, gRPC's `Metadata`. `ctx.route_metadata()` returns
what `#[set_metadata]` declared. Different lifetimes, different writers, adjacent names on the same
type.

Nest has the same collision and never feels it: `Reflector` is an injected service reading off
`getHandler()`/`getClass()`, while wire metadata hangs off the request. They are never two methods on
one object. toni puts both on the context, so toni has to disambiguate by name where Nest disambiguates
by access path.

### The carrier is misfiled, and so is its storage

`RouteMetadata` lives in `http_helpers` while being read on all four transports — the same mistake as
the execution cache riding on `http::request::Parts`, which ADR-0016 removed.

Underneath it is `http_helpers::Extensions`, a second public type of that name distinct from
`context::Extensions`, whose module doc still describes it as "request-scoped data" that "middleware
uses to pass typed data to controllers". That is the other type's job. This one is a synchronous
type-keyed map used as storage here and by `RpcCallInfo`.

## Decision

### The attribute is collected from the impl block and from the method

Every structural macro — HTTP controller, WebSocket gateway, RPC controller, gRPC service — collects
`#[set_metadata]` at both levels and emits the impl-block entries first, the method entries second.

Both are kept, in that order, and `metadata.get::<T>()` answers the last — so method-level wins by
position. That reproduces Nest's first-non-undefined result without a lookup-time search.

`metadata.get_all::<T>()` answers every declaration, least-specific first, which is where Nest reaches
for `getAllAndMerge`. It combines nothing. Nest merges generically on the strength of arrays
concatenating and objects spreading; Rust has no such operation for an arbitrary `T`, and inventing
one guesses wrong on the first type that is a setting rather than a set — a rate limit spread into a
hybrid nobody declared. What it means to combine two values is known where their type is defined,
which is also where the reader that wants them combined lives.

### Declared metadata is `Metadata`, and lives in `context`

`RouteMetadata` becomes `Metadata` in `context`, beside the other things every transport carries.
`HandlerContext::route_metadata` becomes `metadata`.

The attribute is `#[set_metadata]`, so the getter is `metadata()`. `#[set_metadata]` writing something
read back as `route_metadata()` is an asymmetry that has escaped notice while one transport uses it,
and "route" stops being true the moment a WebSocket event or an RPC pattern carries it.

### Wire fields are `headers`, and not by choice

`RpcContext` and `GrpcContext` expose `headers()` and `header(k)` in place of `metadata()` and
`get_metadata(k)`.

This follows from the rename above rather than standing beside it. The two names coexisted for as
long as they differed: `HandlerContext::route_metadata()` and an inherent `RpcContext::metadata()`
never met. The moment the trait method is `metadata()`, an inherent `metadata()` on the same type
shadows it — and only where the receiver's concrete type is known:

```rust
fn generic<C: HandlerContext>(c: &C) -> _ { c.metadata() }  // declared — no inherent candidate
fn concrete(c: &GrpcContext)      -> _ { c.metadata() }     // wire — inherent wins
```

Both compile. `rustc` and `clippy` report nothing at either site. A guard written over
`impl<C: HandlerContext>` would read what `#[set_metadata]` declared, and the identical line in a
handler body, or in a guard specialised to one transport, would read the fields off the wire. That is
the fail-open this ADR opens with in a second form, on `RpcContext` and `GrpcContext` alike.

Most of the transports behind `RpcContext` call these headers already — AMQP, Kafka and NATS by name,
MQTT's user properties by shape. gRPC's specification calls them metadata, which is the one place the
name gives something up. `#[doc(alias = "metadata")]` on both accessors puts them in rustdoc's search
index under the spec term, so the word still finds them.

### The storage primitive is named for what it is, and filed where it belongs

`http_helpers::Extensions` becomes `TypeMap`, keeping its synchronous shape, and its module doc stops
describing the request bag. `context::Extensions` is then the only `Extensions` in the crate.

It moves to `toni::type_map` rather than staying under `http_helpers`, its two users being a declared
map read on four transports and an RPC call descriptor. Leaving it there would repeat one level down
the misfiling this ADR corrects one level up.

## Consequences

- Breaking for `HandlerContext` implementors and every caller of `route_metadata()`.
- Breaking for anything calling `metadata()` or `get_metadata(k)` on an RPC or gRPC context.
- `#[set_metadata]` on an impl block starts having an effect. Nothing that compiles today changes
  behaviour, because nothing collects those attributes now.
- A universal guard begins refusing what it admitted unchecked, on any transport where the annotation is
  present. That is the point of the change and the reason it is worth calling out.
- One type named `Extensions` in the crate rather than two.
- `Metadata::insert` records rather than replaces, and returns nothing. `Metadata::get` is unchanged
  in what it answers and gains a `Clone` bound, every declared type being one already.
- `get_metadata(k)` loses a `get_` prefix the Rust API guidelines discourage, and `headers()` /
  `header(k)` is a plural and its singular rather than two unrelated shapes.
- A gRPC transport file imports `context::Metadata` and `tonic::metadata::MetadataMap` together. The
  names differ enough to compile and not enough to skim, which is worth knowing before it is
  rediscovered.

## Roads not taken

**`handler_metadata()` for the declared side, wire accessors untouched.** Maps onto Nest's
`[getHandler(), getClass()]` targets and is true on all four transports, at the cost of keeping the
`#[set_metadata]` / getter asymmetry that started this.

**Merging parent and child at read time.** Requires walking a target list on every lookup and returning
owned values through `dyn Any`. Insertion order settles it once, at expansion.

