# 0037 — A gRPC handler hands the chain its error, not the status

Status: accepted

## Context

On HTTP, RPC and WebSocket the handler's error type belongs to toni, and each one has a variant whose
job is holding the user's value:

```rust
// toni/src/errors/http_error.rs
AppError(Arc<dyn Error + Send + Sync>),
```

A handler returns `Err(OutOfStock { … })`, the `From<E: Error>` blanket parks it there, and the
dispatcher takes it back out on the way to the chain, so `#[catch(OutOfStock)]` downcasts and matches.

gRPC has no such variant, because the return type is tonic's:

```rust
async fn create(&self, request: Request<CreateOrderRequest>)
    -> Result<Response<CreateOrderResponse>, tonic::Status>
```

By the time the error reaches toni it is a code and a message. `tonic::Status` has no extensions bag
to carry the original, so the chain was handed the flattened status and a catcher could only match on
what survived. The framework's own example did the visible thing: `error.to_string().contains(
"invalid-qty")`.

ADR-0036's kind mapping fixed the *code* — a `Conflict` is `ABORTED` on every service. It could not
fix the identity.

## Decision

**The error travels beside the answer when it cannot travel inside it.** `GrpcContext` already exists
per call, and its extensions are toni's own, so `fail` parks the domain error there and answers with
the status its `kind()` maps to:

```rust
let ctx = GrpcContext::of(request.extensions()).expect("a toni-dispatched call");
Err(ctx.fail(OutOfStock { item }))                       // or: reserve(&item).fail_with(&ctx)?
```

The `#[grpc_methods]` wrapper takes the parked error on its way to the chain and hands that over
instead of the wrapped status. A `#[catch(OutOfStock)]` handler matches on gRPC exactly as it does on
the other three, and an unclaimed failure renders the status it always would have.

**`to_status` stays for the calls that have no context to hand over.** The orphan rule forbids toni
from writing `From<YourError> for tonic::Status`, so a user who wants bare `?` writes that impl in
their own crate — and an impl has no context. `to_status` is the mapping for that path, and for any
helper below the handler. The rule that separates them: inside a handler, `fail`; without a context,
`to_status`, and the chain sees a status.

## Consequences

- gRPC's chain sees domain types, so an application's error handling is one set of catchers rather
  than three plus a substring match.
- Two ways to produce a status from an error, with one wrong default: a user who reaches for
  `to_status` in a handler and registers `#[catch(MyError)]` finds it never fires. The rule above is
  documentation's job; the alternative — dropping `to_status` — takes away the only path that works
  where no context exists.
- The parked error is removed when read, so a call that fails twice keeps the last failure, which is
  the one the returned status describes.
- Nothing changes on the wire. The status a caller receives is the same either way; the difference is
  which handlers on the server can act on it.

## Roads not taken

**Carrying the error in `tonic::Status`.** Nowhere to put it: `Status` holds a code, a message and
wire metadata, with no typed slot. Encoding it into `details` would put an internal value on the wire.

**Making `to_status` type-preserving.** It would need the execution without being handed it, which is
ambient per-task state — declined in ADR-0016 and the reason a handler is given what it needs.

**Changing the handler signature so toni owns the error type.** The signature is the tonic trait's;
owning it would mean generating a parallel trait and giving up `tonic-build` output as the contract.
ADR-0034's line applies: the framework does not take ownership of what the ecosystem already defines.
