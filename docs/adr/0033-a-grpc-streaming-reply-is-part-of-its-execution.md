# 0033 — A gRPC streaming reply is part of its execution

Status: proposed

## Context

ADR-0021 fires the cancellation token when a scoped body or stream is dropped before its last item,
and ADR-0032 gave RPC the same treatment. gRPC was excluded from both, by omission rather than by
argument: server-streaming and bidi replies are handed to tonic exactly as the handler built them,
so the execution ends when the method returns and nothing feeding an abandoned reply learns.

Two facts decide the shape of the fix.

**A gRPC reply's type is not the framework's.** HTTP wraps `toni::Body` and WebSocket wraps
`WsHandlerOutput::Stream`, both types the framework defines and can substitute. A gRPC reply is
typed by the tonic trait's associated stream type, which the user's impl defines. A value cannot be
wrapped without changing its type.

**A gRPC handler cannot reach its context.** The signature is tonic's and carries no context
parameter, so guards, interceptors and error handlers receive one and a handler does not. The
generated method puts the execution's extension bag on the request, which is how a handler reads
what a guard wrote, and the bag is all it puts there. The cancellation token lives on the context.
Firing it while it is unreachable would signal into a room with nobody in it.

The second fact is why this ADR exists rather than a paragraph appended to ADR-0021: covering the
tail requires deciding what a gRPC handler may see.

## Decision

### The context rides the request

`#[grpc_methods]` inserts the `GrpcContext` into the tonic request's extensions alongside the bag,
and `GrpcContext::of(request.extensions())` reads it back. A handler that wants to stop feeding an
abandoned reply selects on `ctx.cancellation().cancelled()`, the same idiom the other three
transports use through a context parameter.

This also gives a gRPC handler the declared metadata of ADR-0020, which it could not read before.
That is a consequence of the carrier, not a separate decision: a context on the request is the whole
context.

### The wrapper declares its own associated stream types

The generated wrapper stops aliasing the user's associated types:

```rust
type WatchProgressStream = ScopedGrpcStream<<OrdersService as Orders>::WatchProgressStream>;
```

`ScopedGrpcStream` holds the context and fires its token on a `Drop` before the inner stream answers
`None` — `ScopedStream` and `ScopedRpcStream`, at the seam gRPC leaves open. It is generic over the
inner stream and names no tonic type, so `grpc_runtime` stays tonic-free.

Carrying the reply to the wrapper's type needs no per-method code. A two-impl trait resolves it by
the target type alone:

```rust
impl<T> IntoScoped<T> for T                              // a message is already what the trait wants
impl<S: Stream> IntoScoped<ScopedGrpcStream<S>> for S    // a stream is wrapped on the way out
```

The impls cannot overlap, since that would require `S == ScopedGrpcStream<S>`. Every generated
method emits the same line, and unary methods resolve to the identity impl at no cost.

### A method is streaming under either spelling of its signature

An attribute macro reads spellings, not types. `Self::WatchProgressStream`, an alias, and the
expansion behind them are one type to the compiler and three token sequences to the macro, and a
signature may legally use any of them. Both spellings that occur in practice are read.

A response type written `Self::SomeStream` is the direct evidence, and it is what tonic-build
declares in the trait — 40 occurrences in this repository, no counterexamples. Where a signature
names the concrete type, the method is paired with its associated type by name: tonic-build derives
`watch_progress` and `WatchProgressStream` from the one proto identifier and emits the associated
type only for the methods that stream, so the pairing is a fact about the trait rather than about
the signature. The wrapper's signature then restates that payload as `Self::SomeStream`, being
generated text under no obligation to copy the user's.

Neither signal reaches a hand-written trait whose associated type is named off that convention and
whose method also avoids `Self::` — `type Feed` beside `async fn watch(…) -> Response<Feed>`. Its
reply passes through unwrapped, which is the behaviour before this change. Reading tonic-build's
naming is what the macro already does to find the server type from the trait name.

## Consequences

- A gRPC handler can read its execution: cancellation, declared metadata, the execution cache, the
  method path and wire headers. The doc comment stating that only enhancers see a context is no
  longer true and is corrected.
- A streaming reply outlives its handler with the execution attached, so an extension-bag read from
  inside a gRPC stream reaches the execution's bag rather than a detached one — which ADR-0016 listed
  as a consequence and which was only true where the handler had cloned a handle out first.
- The token fires when a caller resets a stream or its connection dies. It does not fire at
  shutdown: tonic's per-connection tasks outlive the dropped serve future, so a reply still being
  served survives the drain deadline and the application reports shutdown complete while it runs.
- A streaming reply costs one `Box::pin` and one `Arc` clone per call, and one branch per item.
  Unary replies cost nothing.
- A service is covered under either spelling of its streaming signatures, and the wrapper's own
  signature stops being a copy of the user's for the methods it re-types.

## Roads not taken

**Wrap the encoded response body in a tower layer.** The context rides out on
`tonic::Response::extensions` and the adapter holds it until the body's last frame — no
associated-type rewrite, no classifier, uniform over every call mode. Two properties rule it out.
A gRPC reply ends with status trailers and hyper stops polling once it has them, so a body wrapper
cannot read the end from `Poll::Ready(None)` the way its three siblings do; read that way, every
completed call reports as abandoned. Trailers also carry a clean finish and a failed stream alike,
which leaves the two indistinguishable where ADR-0032 treats an item carrying an error as an
abnormal end. The seam additionally spends a dependency on tonic preserving response extensions
through its own response mapping.

**Reject the concrete spelling.** Rewriting every associated type and leaving the mismatch to the
compiler produces `E0053` on the user's own signature, whose `help` suggests writing a
framework-internal type into their service. Emitting a macro error instead reads better and still
refuses a signature the language accepts. The name pairing identifies the method without either.

**`tonic::Response::map`.** Shorter than `into_parts`/`from_parts` and `#[doc(hidden)]`, which is
tonic saying not to depend on it.

**A `deadline()` from `grpc-timeout`.** Still a different feature, as ADR-0021 recorded.
