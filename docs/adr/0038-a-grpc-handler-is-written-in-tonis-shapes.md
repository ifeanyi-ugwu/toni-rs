# 0038 — A gRPC handler is written in toni's shapes, and the macro writes tonic's

Status: proposed

## Context

On HTTP, RPC and WebSocket a handler takes what it asks for and answers with what it has:

```rust
#[get("/orders/{id}")]
async fn get(&self, Path(id): Path<u64>) -> Result<Body, OrderError> { … }
```

On gRPC the signature belongs to tonic, and the handler spells out the transport:

```rust
async fn create(&self, request: tonic::Request<CreateOrderRequest>)
    -> Result<tonic::Response<CreateOrderResponse>, tonic::Status>
```

Every gap gRPC has follows from that one fact. Extraction never reached it, so ADR-0023's
`FromContext` covers three transports. A domain error cannot be returned, so ADR-0037 had to park it
on the execution to keep its type. The associated stream types are declared by hand. The context is
fetched off the request rather than asked for. The constructor lives in a second impl block, because
the first one belongs to the trait.

None of it is reachable while `#[grpc_methods]` *wraps* an impl written against tonic's trait: the
macro does not own the signature, so it cannot change what a handler says.

## Decision

**`#[grpc_methods(proto::Trait)]` on an inherent impl writes the trait impl.** The handler is an
ordinary toni handler, and the block holds the constructor beside it, the way `#[routes]` and
`#[patterns]` do:

```rust
#[grpc_methods(greeter_server::Greeter)]
#[use_error_handlers(NoNameHandler)]
impl GreeterService {
    #[new]
    pub fn new() -> Self { Self {} }

    #[grpc_method]
    async fn greet(&self, Payload(req): Payload<GreetRequest>, ctx: &GrpcContext)
        -> Result<GreetReply, NoName>
    {
        if req.name.is_empty() { return Err(NoName); }
        Ok(GreetReply { message: format!("{} on {}", req.name, ctx.method()) })
    }
}
```

**A streaming reply is a stream of the handler's own types.** `#[grpc_stream]` marks it:

```rust
#[grpc_stream]
async fn greet_many(&self, Payload(req): Payload<GreetRequest>)
    -> Result<impl Stream<Item = Result<GreetReply, NoName>> + Send + 'static, NoName>
```

The macro declares the associated type the trait asks for — `greet_many` pairs with
`GreetManyStream`, the pairing tonic-build makes from one proto identifier — as a boxed stream over
`Result<Reply, Status>`, and boxes the handler's stream into it. The item type is read from the
`Item =` binding, which is why the reply is written as `impl Stream<Item = …>` rather than as a
concrete stream type.

The two errors a stream can carry take different routes. The one that prevents the stream from
opening is an ordinary handler error and reaches the chain like any other. An item's error arrives
after the answer has begun, so it maps to the code its kind means and goes on the wire — the split
ADR-0032 records for an RPC reply stream.

**The caller's stream arrives as `Inbound<T>`.** A client-streaming or bidirectional rpc hands the
handler a stream of the message type:

```rust
#[grpc_method]
async fn greet_all(&self, mut inbound: Inbound<GreetRequest>) -> Result<GreetReply, NoName>
```

Its items fail with a `GrpcStatus` rather than tonic's, so a handler reading one names nothing from
the wire crate; the conversion happens where the macro unwraps the request. Which of the four call
shapes a method serves is read entirely from its own signature — `Payload<T>` or `Inbound<T>` for the
request, `#[grpc_method]` or `#[grpc_stream]` for the reply — so bidirectional is the two streaming
answers together rather than a third marker.

**What a handler takes.** `Payload<T>` or the message written bare, `Inbound<T>` for the caller's
stream, `Extensions` for the execution's bag, `&GrpcContext`, and `tonic::Request<T>` for a handler
that wants the wire shape — trailers, the peer address, the metadata map as it arrived. A parameter
naming none of those is read as the request message, the way an RPC handler spells its payload; a
misspelled extractor lands there and fails as a type mismatch against the proto message.

The raw request matters more than it looks: it is what keeps this form from being a subset of what
the trait impl could express, so nothing is stranded when that form goes.

**A trait that names its stream differently says so on the method.** `#[grpc_stream]` reads the
associated type from the method — `greet_many` pairs with `GreetManyStream` — which holds because
tonic-build derives both from one proto identifier. `tonic_build::manual` sets the Rust name and the
route name independently, so a `watch` there may declare `StreamProgressStream`. The attribute
carries it: `#[grpc_stream(StreamProgressStream)]`.

**The proto trait is named, not inferred.** A trait impl states it in its header, which is where the
macro reads it today; an inherent impl has no header, so inference would mean guessing a module path
and a trait name from the struct's identifier. That guess breaks the first time a service is named
for its domain rather than its proto, and it fails as an error naming a path the author never wrote.

**A handler keeps its body under a hidden name.** The generated trait method calls
`__toni_grpc_greet`, not `greet`. Both impls carrying the same name would leave the call resolving by
inherent-first precedence — correct until something shifts, and then it is the generated method
calling itself.

**The generated impl is fed to the machinery that already existed.** The wrapper, the enhancer
resolution, the declared metadata and the panic recovery read a trait impl; they cannot tell whether
a person or the macro wrote it. Guards, interceptors and `#[catch]` handlers behave exactly as they
did.

**An error is mapped and parked.** The generated method converts through `grpc_code` and leaves the
value on the execution, so `#[catch(MyError)]` matches on this transport as it does on the others —
ADR-0037's mechanism, now reached without the handler calling anything.

## Consequences

- All four call shapes are expressible: unary, server streaming, client streaming and
  bidirectional.
- A handler's error type implements `toni::Error`. `GrpcStatus` does not, and cannot: it is what a
  `toni::Error` maps into, and implementing both sides would collide with that blanket. A handler
  wanting a code no `ErrorKind` reaches — `FailedPrecondition`, `OutOfRange` — takes the raw request
  and answers `tonic::Status` itself.
- `Validated<Payload<T>>` is not among the parameters. Proto messages are generated, so there is
  nowhere to hang the `#[validate]` attributes it reads; validation on this transport is a check
  inside the handler.
- The trait-impl form is unchanged and still compiles, so a service written against tonic's
  signatures keeps working.
- A streaming reply is boxed once per call. The associated type belongs to the macro rather than the
  handler, which is what lets the wrapper redeclare it as `ScopedGrpcStream` — so a reply the caller
  abandons fires the execution's token here as it does for a hand-written impl (ADR-0033).
- **toni now tracks tonic-build's generated trait** — `#[async_trait]` versus native async in traits,
  argument shapes, associated-type naming. ADR-0034 ruled that the framework does not own what the
  ecosystem already defines, weighing a client module that would have wrapped a single constructor
  call. The weight is different here: what the wrapping buys is one handler shape across four
  transports, and what it costs is following one crate's generated output.
- `#[grpc_method]` is a marker the enclosing macro strips, like `#[stream(…)]`. It is not exported and
  needs no import.

## Roads not taken

**Inferring unary versus streaming from the return type.** ADR-0033 already reads streaming out of
spellings and needed three signals plus `#[stream(…)]` as an escape hatch, because a macro sees
tokens rather than types. A fourth heuristic would be built on the same sand; a marker costs one word
and cannot be misread.

**Supporting both a marker and inference.** Two spellings for one fact, two code paths, two
diagnostics, and a decision every author makes once for nothing.

**Keeping the handler's own name in both impls.** It works by inherent-first resolution, and the
failure mode when it stops working is silent recursion rather than a compile error.
