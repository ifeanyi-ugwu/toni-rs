# 0042 — A gRPC handler asks the type what the wire carries

Status: accepted

## Context

[ADR-0038](0038-a-grpc-handler-is-written-in-tonis-shapes.md) made `#[grpc_methods]` write the proto
trait impl from handlers spelled in toni's shapes. To write it, the macro has to know the message
type: the trait's method is declared `request: tonic::Request<GreetRequest>`, and that token has to
appear in the generated signature.

It read the type off the parameter's name. `Payload<T>` and `Inbound<T>` were recognised by their
last path segment, and anything else was taken to be the message itself. A proc macro runs before
name resolution, so `use toni::extractors::Payload as P` gives it the identifier `P` and nothing
else — the alias fell to the bare-message case, and the generated method declared
`tonic::Request<Payload<GreetRequest>>`. That fails, which is
[ADR-0040](0040-a-name-the-framework-reads-is-backed-by-a-type.md)'s rule met, but it fails as an
`E0053` reported on the generated method's own span — the `#[grpc_methods]` attribute — rather than
on the parameter.

gRPC was also the last transport reading parameters this way. HTTP, RPC and WebSocket each extract
every parameter through `FromContext`, and RPC's own name list went with
[ADR-0041](0041-an-rpc-handler-takes-what-every-handler-takes.md).

## Decision

**The request is the first parameter, and its type says what the wire carries.** Position is a fact
the macro can read without resolving anything; the message type is asked of the type itself:

```rust
pub trait GrpcRequest {
    /// What the wire carries — the message, or a stream of them.
    type Arg;
    /// The request the proto trait's method is declared with.
    type Request;
    fn from_request(request: Self::Request) -> Self;
}
```

The generated signature is a projection, so the macro never names the message:

```rust
async fn greet(&self, request: ::tonic::Request<<Payload<GreetRequest> as GrpcRequest>::Arg>)
    -> Result<::tonic::Response<GreetReply>, ::tonic::Status>
```

`Payload<T>` projects to `T`, `Inbound<T>` to `tonic::Streaming<T>`, and `tonic::Request<T>` to `T`.
rustc normalizes each against the trait's own declaration, aliases included, and a type that is not
a request shape fails as an unsatisfied `GrpcRequest` bound reported at the parameter.

**Every other parameter is a `FromContext<GrpcContext>`,** which is what the other three transports
do. `&GrpcContext` is passed through rather than extracted, the one name still read on any of them,
and it is backed by being passed at that type.

**The bare-message form is removed.** It cannot survive the trait: `impl<T: Message> GrpcRequest for
T` overlaps `impl<T> GrpcRequest for Payload<T>`, because coherence must assume an upstream crate
could implement `Message` for `Payload<T>`. Specialization would resolve it and is unstable. A
handler takes `Payload<T>`, which is the spelling every other transport now uses.

**The trait lives in `toni-grpc`.** Its impls name `tonic::Request`, which toni core does not
depend on; a trait declared in core could not be implemented in `toni-grpc` for `Payload<T>`,
since both would be foreign there. Generated code therefore names `::toni_grpc::GrpcRequest`, so a
crate that writes `#[grpc_methods]` depends on `toni-grpc` at compile time rather than only to
serve.

## Consequences

- An aliased or re-exported extractor works, and a wrong one fails at the parameter naming the
  types that do implement `GrpcRequest`.
- `classify_param`, `claim_request`, `HandlerParam` and `RequestKind` are gone. Which of the four
  call shapes a method serves is read from the request parameter's projection and the presence of
  `#[grpc_stream]`, neither of which is a name.
- Two parameters cannot both take the request: only the first is one, and a second `Inbound<T>`
  is extracted through `FromContext<GrpcContext>`, which it does not implement. The diagnostic is
  ADR-0041's.
- The request must come first. Every handler in the tree already wrote it there.
- `#[grpc_stream]` stays. It marks the reply, which no parameter type can tell the macro.
- `Payload<T>` reads as an extractor on the other three transports and as a request shape here. The
  spelling is shared and the mechanism is not, which is the price of gRPC being the one transport
  whose signature toni does not own.

## Roads not taken

**A `#[request]` marker instead of a position.** It buys free ordering and costs an attribute on
every handler, to express something the first parameter already says.

**Keeping the bare form behind a second trait.** Two traits, two diagnostics, and the same overlap
question the moment either has to answer for a wrapper.

**Leaving gRPC on name-matching.** It is backed and correct. What it costs is a diagnostic pointing
at an attribute, a fourth way of reading a parameter, and a second implementation of every
parameter feature the other transports already have.

**Extracting the request from the context like every other parameter.** `Payload<T>` is a
`FromContext` on the other three transports and a `GrpcRequest` here, which is one spelling with two
mechanisms. Unifying it means shrinking this trait to `type Arg` alone, putting `request.into_inner()`
into the `GrpcContext` type-erased, and extracting the request through `FromContext<GrpcContext>`
like the parameters after it.

It buys the spelling and loses on every other count. A typed move becomes a runtime downcast.
`tonic::Request<T>` as a parameter stops working, because the context is built *from* that request's
extensions and cannot hand it back. And the positional rule survives regardless: the macro must name
the message type in the signature it writes, and only a parameter can tell it which — erasing the
value does not change where the type comes from.

`GrpcContext` carries the method, the headers, the peer address and the deadline, not the message.
That is what the comparison turns on, and it is why the request is handed over rather than looked
up.
