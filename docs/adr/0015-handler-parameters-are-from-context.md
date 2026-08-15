# 0015 — Handler parameters are extracted from the handler's context

Status: accepted

## Context

An extractor was a name. `Path`, `Query`, `Json` and the rest were entries in a table inside the
controller macro, matched by the last segment of the written type. A type absent from that table
became `Unknown`, and `Unknown` was guessed at: it was handed the real request body when nothing
else wanted it, and an empty body otherwise, so that an aliased import of a body extractor behaved
like the un-aliased name while several custom metadata extractors could still share a handler.

Nothing about a type recorded which transports it was valid for. `Path<u32>` in a WebSocket handler
was not a mistake anything could detect — the WebSocket macro had no table at all, and passed every
handler the same two values.

The three transports had grown three answers to the same question. HTTP classified parameters and
supported any number in any order. WebSocket called `self.method(client, message)`, so a handler
declared both whether or not it used either. RPC called `self.method(payload, ctx)`, inspecting the
first parameter to decide whether to deserialise it.

## Decision

One trait, generic over the context an extractor reads from:

```rust
pub trait FromContext<C: HandlerContext>: Sized {
    type Error: Display;
    fn extract(ctx: &mut C) -> impl Future<Output = Result<Self, Self::Error>> + Send;
}
```

An extractor declares which contexts it works with by which impls it carries. `Path<T>` carries
`FromContext<HttpContext>`; `WsClient` carries `FromContext<WsContext>`. Reaching for one in the
wrong handler is an unsatisfied bound, reported where it is written, against the type that is
missing an impl.

One generic trait rather than one per transport, because a transport-agnostic extractor is then
written once — `impl<C: HandlerContext> FromContext<C> for Extensions` covers all of them, where
three separate traits would need three impls that could drift apart.

Extraction reads from the context, which is what allows the trait to be generic over one. A handler
therefore receives its context and nothing beside it: `Route::execute` and
`GatewayTrait::handle_event` both take only `&mut C`. What used to be passed alongside — the
request, the client, the message, the event — all live on the context, so passing them as well was
duplication that only the fixed signatures concealed.

Nothing is moved out of a shared local during extraction, so the order parameters are written in
carries no constraint. The parts-before-body ordering the HTTP macro used to impose is gone.

The error type belongs to the extractor rather than the context. `HandlerContext` is used as
`dyn HandlerContext` by `ErrorObserver` and the observer fan-out, and an associated error type on it
would not be dyn-compatible. Each transport's macro knows its transport and renders accordingly.

### Two shorthands, and why one of them is not a blanket

`FromRequestParts` — sync, metadata only — keeps its place and gains `FromContext<HttpContext>`
through a blanket impl. Extractors written against it need no change.

`FromRequest` — async, takes the whole request — pairs with a small `FromContext` impl calling
`extract_body`. This is deliberately not a second blanket: two blanket impls over `T` overlap
whatever their bounds, while a blanket and a concrete impl do not. A marker trait to distinguish
them does not help, because the marker's blanket is still a blanket.

### `Payload<T>` is a WebSocket and RPC spelling

On those transports the thing a handler wants is *the message*, and no name for it existed.
On HTTP it is *the body*, and `Json<T>`, `Bytes`, `Body<T>` and `BodyStream` already name it more
precisely than a single word could. Uniformity of shape, not of every noun.

### The body is read once, and says so twice

A request body may be a stream, so there is nothing to give a second reader. Two enforcement layers,
because neither covers the other's set:

- The handler macro rejects a second body extractor at compile time when it recognises both types,
  naming the two parameters that collide.
- An extractor it does not recognise — anything custom — finds the body gone and fails with
  `BodyExtractionError::AlreadyRead`, naming itself, and logs at error level: the request was
  well-formed and the fault is the application's.

## Consequences

- Breaking for custom body extractors, which need a `FromContext` impl added — six lines, and
  `extract_body` is exported for the purpose. Custom metadata extractors carry forward untouched.
- Breaking for `Route` and `GatewayTrait` implementors outside the macros. Four `Route` impls and one
  `GatewayTrait` impl are in-tree, all in the GraphQL crates.
- Existing WebSocket handlers taking `(WsClient, WsMessage)` classify to what they already mean and
  keep compiling. The pair becomes the most common choice rather than the only signature.
- An enhancer that reads the body used to leave the handler an empty one; it now leaves an extraction
  failure naming the extractor that came up short. A handler that silently received nothing now
  fails visibly.
- gRPC is excluded. `#[grpc_methods]` re-emits a signature the tonic trait dictates, so joining would
  mean generating that impl from a free-signature method — a larger change with no bearing on the
  three transports here. gRPC handlers reach the extension bag through the tonic request instead.
- WebSocket handlers can now read `route_metadata()` from their context, and it is always empty:
  `#[set_metadata]` is not wired for gateways on either level. The access is real, the value is not
  yet.
