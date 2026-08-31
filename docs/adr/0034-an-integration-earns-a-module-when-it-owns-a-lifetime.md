# 0034 — An integration earns a module when it owns a lifetime

Status: proposed

## Context

Six database integrations ship a module. GraphQL ships a module. Config ships a module. A gRPC
client is a provider:

```rust
providers: [provider_factory!(OrdersClient<Channel>, || {
    OrdersClient::new(Channel::from_static("http://orders:50051").connect_lazy())
})]
```

Nothing states why the line falls there. NestJS hands out a typed gRPC client through
`ClientsModule.register(...)`, so the absence of an equivalent reads as a missing capability rather
than as a decision, and the next integration added to this framework is decided by whichever
neighbour its author happened to read.

Guessing is not symmetric. A module costs a crate or a builder, a `for_root`, a module identity
(ADR-0029), a named variant once someone needs two of them, and a page of documentation. A provider
that should have been a module costs one refactor. The default therefore wants stating, not
inferring.

## Decision

**An integration earns a module when something has to own a lifetime the container cannot infer from
a constructor expression.** Concretely, when any of these is true:

- a connection or pool whose construction takes configuration beyond a URL
- a startup check, so unreachability is refused at `create` rather than at first use (ADR-0026)
- a shutdown hook with work to do beyond dropping
- a health indicator sharing the registered connection
- registration under a token the framework reserves, so two of them can coexist (`for_root_named`)

None of those true, it is a provider. `provider_factory!` registers it under its own type, `#[inject]`
resolves it, and the container owns when it is built and when it drops — which is the whole of the
lifetime there is.

The database modules own all five. The GraphQL module owns route registration at bind, which is the
same kind of ownership wearing different clothes. A client owns none: one expression, no pool, no
check by design, and dropping the channel is closing it.

### Two decisions already made are what put a client on that side

**Connect lazily, so there is no startup to own.** The RPC client transports document no I/O in a
constructor, and `Channel::connect_lazy` matches it. A peer that is not up cannot fail a boot, and
reaching a dead peer is an ordinary handler error. A module offering a reachability probe would
contradict this rather than extend it.

**No ambient per-call state, so there is no propagation to own.** Forwarding an inbound
request's headers to an outbound call is the one thing a client module could plausibly automate, and
it needs state a call can reach without being handed it. ADR-0016 declined ambient context and settled
that a handler is given what it needs. Propagation is therefore a per-call decision made with the
context the handler already holds.

## Consequences

- A client integration is documentation and an example, not a crate. `examples/grpc_client.rs` is the
  worked one.
- What a client module would have offered is reachable without it: a configuration-driven URL through
  a factory closure that takes injected dependencies, and two clients of one type through string
  tokens — which is where `for_root_named` had to arrive for the database modules anyway.
- The next integration has a test to apply rather than a neighbour to imitate.
- A module added for something owning no lifetime is a second spelling of `provider_factory!`, and
  the reviewer now has grounds to say so.

## Roads not taken

**A `GrpcClientModule` mirroring Nest's `ClientsModule`.** Nest's module earns its place by owning
transport selection, a proto path, loader options and channel credentials — configuration that has
somewhere to live. tonic's generated client already holds all of it, and toni's version would wrap one
constructor call in a builder that reads it back out.

**A reachability check on clients.** Coherent, and contradicts the lazy connect that keeps startup
independent of peers, so it would have to be opt-in. ADR-0026 already has the shape if someone wants
to refuse a boot when a dependency is down. Not built until asked, and recorded here so the answer is
not invented twice.

**Automatic metadata propagation.** Needs ambient per-task state; see above.
