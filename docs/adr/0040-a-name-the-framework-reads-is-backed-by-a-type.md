# 0040 — A name the framework reads is backed by a type it checks

Status: proposed

## Context

Several macros decide what a handler parameter means by reading the last path segment of its
written type. `#[grpc_methods]` classifies `Payload`, `Inbound`, `Request`, `Extensions` and
`&GrpcContext` that way; `#[routes]` classifies the HTTP extractors the same way to decide which
parameter reads the body.

A name is not a type. `use toni::extractors::Payload as P` gives a parameter whose segment reads
`P`, and a handler is free to define its own `Payload`. Either way the classification is wrong, and
the question is what happens next.

## Decision

**A name the framework reads is backed by a type it checks.** The name buys the diagnostic; the
type decides the program. Concretely, every classification must be followed by generated code that
fails to compile when the name lied — the parameter is passed to something typed, or bound by a
trait only the real type satisfies.

Where that holds today:

| Classification | What catches a lie |
| --- | --- |
| `Payload<T>` / `Inbound<T>` on gRPC | the parameter receives `toni::extractors::Payload` / `Inbound`, built by the generated method |
| a bare type on gRPC | it becomes the trait method's request type, so a wrong one fails against the proto trait's signature |
| any parameter on RPC or WebSocket | it is extracted through `<T as FromContext<RpcContext>>` / `<T as FromContext<WsContext>>`, so a type with no impl has nothing to extract through |
| `&GrpcContext` / `&RpcContext` / `&WsContext` | the context is passed by reference at that type |

**Where a name cannot be backed, the type declares the fact instead.** HTTP's one-body rule was the
exception: `#[routes]` decided which parameters read the request body by matching their last path
segment against a table of extractor names. Nothing downstream caught a miss, because reading the
body is a runtime take from a shared context rather than a move, so a name the table did not know —
an alias, a custom extractor, a `#[body]` marker — was simply not counted and the second reader
failed at request time.

`FromContext` now carries the fact:

```rust
pub trait FromContext<C: HandlerContext>: Sized {
    type Error: fmt::Display;

    /// Whether extracting this consumes what it reads, leaving nothing for a
    /// second extractor.
    const CONSUMES: bool = false;

    fn extract(ctx: &C) -> impl Future<Output = Result<Self, Self::Error>> + Send;
}
```

`#[routes]` emits one assertion per pair of parameters, reading the const off each written type:

```rust
const _: () = {
    assert!(
        !(<Json<Dto> as FromContext<HttpContext>>::CONSUMES
            && <MyBody as FromContext<HttpContext>>::CONSUMES),
        "`dto` and `raw` both read the request body, and it can only be read once. …"
    );
};
```

The pair rather than a sum, because a sum can say only that two of several parameters read the
body while a pair names both. The const is defaulted, so an existing extractor keeps compiling and
reads `false`; `Option<T>` and `Validated<E>` forward what they wrap; a `#[body]` marker is counted
as the `Body<T>` it extracts through.

**The runtime check stays, because it covers what no signature can.** `take_body` answers
`BodyAlreadyRead` when the body has gone, and a *guard* that read the body is not a handler
parameter — no assertion over a signature can see it. That is the division: the assertion covers
parameters, and `BodyAlreadyRead` covers everything else, including an extractor whose author did
not set the const.

## Consequences

- Diagnostics are what a *backed* name-match delivers, not correctness. A proposal to make one of
  those type-driven is answered by asking which program it would reject that is accepted today;
  where the answer is none, the work is a better message. The one-body rule was the case where the
  answer was not none.
- Two handlers with the same parameter names committing the same violation produce one diagnostic,
  because the assertion messages are identical and rustc deduplicates them.
- The diagnostic an unrecognised name produces on gRPC names both types — *expected
  `GreetRequest`, found `Payload<GreetRequest>`* — while pointing at the attribute rather than the
  parameter. `E0053` is reported on the generated method's own span, which `parse_quote!` sets at
  the call site; spanning the request type on the parameter does not move it.
- The rule is a constraint on new classifications rather than a description of them. Adding a name
  to one of these lists means naming the check that catches a parameter which is not what its name
  says.

## Roads not taken

**Classifying by trait rather than by name.** The macro must know the request's message type at
expansion time — it writes the trait signature `tonic::Request<T>` from it — and a trait bound is
resolved long after. Asking the type system would mean putting the decoded message behind a
runtime downcast, turning compile errors into request-time ones.

**Leaving the one-body rule to `BodyAlreadyRead` alone.** One mechanism, uniform message, no
compile-time table — but `Json<T>` beside `Bytes` is a mistake that is always a mistake, and
answering it with a 400 at request time is worse feedback than a compile error for the sake of
having one mechanism rather than two.

**Rejecting an unrecognised name instead of reading it as the request.** The bare-message form is
what RPC handlers have always spelled, and on gRPC it is the shorter half of the pair with
`Payload<T>`. Removing it to make a misspelling louder costs more than it buys.

*Superseded on RPC by [ADR-0041](0041-an-rpc-handler-takes-what-every-handler-takes.md).* The cost
weighed here was the misspelling alone. What the form also bought was a fork, and a name list to
select it, which is what made a handler's own `Payload` type read as the framework's.
