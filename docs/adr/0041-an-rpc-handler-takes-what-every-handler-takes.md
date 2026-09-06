# 0041 — An RPC handler takes what every other handler takes

Status: accepted

## Context

ADR-0023 collapsed extraction to one trait, and [ADR-0040](0040-a-name-the-framework-reads-is-backed-by-a-type.md)
states that a name the framework reads must be backed by a type it checks. RPC meets the second rule
and not the first. `#[patterns]` forks on the parameter's written name:

```rust
if is_known_extractor(ty) {                     // RpcData | Extensions | Payload | Validated
    <Ty as FromContext<RpcContext>>::extract(ctx).await
} else {
    ctx.data().parse::<Ty>()                    // the bare form
}
```

The bare form is why the list exists. Without it every parameter would be a `FromContext<RpcContext>`
and there would be nothing to fork on — which is exactly WebSocket, where a handler's parameters are
free-form extractors and the only name read is `&WsContext`.

The list costs more than the spelling saves.

A handler's own type sharing a framework extractor's name is read as the framework's. `struct Payload
{ id: u32 }` as a parameter compiles to `<Payload as FromContext<RpcContext>>::extract`, and the
failure asks the author to implement a trait they have never heard of rather than to rename their
type. The name that caused it appears nowhere in the message.

An aliased extractor takes the other branch. `use toni::extractors::Payload as P` then `P<Order>`
falls to the bare form, and the failure is that `Payload<Order>` does not implement
`DeserializeOwned` — a fact about a trait the author never mentioned.

And the fork bounds what the transport can grow into. A parameter that goes through the bare branch
has no `FromContext` impl, so nothing about it can be read off a type — including
[`CONSUMES`](0040-a-name-the-framework-reads-is-backed-by-a-type.md), which is how HTTP counts the
readers of a resource there is one of. RPC has no such resource today because a payload is buffered.
A client-streamed request would be the first, and the fork is what would stop it being counted.

## Decision

**The bare form is removed.** Every parameter of an RPC handler is a `FromContext<RpcContext>`, and
the message is taken through an extractor that says so:

```rust
#[message_pattern("orders.create")]
async fn create(&self, Payload(order): Payload<CreateOrder>) -> Result<OrderId, RpcError>
```

`Payload<T>` deserialises the call's data into `T`, which is what the bare form did — the difference
is that the parameter says where its value comes from instead of the macro inferring it from what
the name is not.

`is_known_extractor` goes with it. The only name `#[patterns]` reads is `&RpcContext`, which is
passed at that type and so is backed, and which every other transport reads the same way.

**`FromContext` says what to do when a type is not an extractor.** The failure a removed bare form
produces is a missing impl, and `#[diagnostic::on_unimplemented]` makes it name the two ways out
rather than the one the compiler would guess.

## Consequences

- A handler taking its message bare no longer compiles. The fix is one wrapper per parameter, and
  the diagnostic names it.
- RPC and WebSocket now have the same rule, stated the same way: every parameter is an extractor,
  one name is read, and it is backed.
- A custom extractor and a framework one are the same thing to the macro. Neither is on a list.
- `Validated<Payload<T>>` is unchanged — it was already the extractor branch.
- The transport can grow a consumed request without first removing a fork.

## Roads not taken

**Keeping the bare form and dropping the list with a blanket impl.**
`impl<T: DeserializeOwned> FromContext<RpcContext> for T` collides with the impls for `Payload<T>`,
`RpcData` and the rest, and specialization is unstable. Making those transparently `Deserialize`
does not save it: `Extensions` is read from the context and not from the payload, so it can never
join a payload-shaped blanket.

**Keeping the bare form and accepting the fork.** It reads well in a signature, and that is the
whole case for it. Against: two ways for a parameter to get its value, a name list that decides
which, a shadowing diagnostic that misdirects, and a bound on what the transport can be given later.
