# 0030 — One source builds every dispatch target, and enhancer tokens resolve at create

Status: accepted

## Context

ADR-0017 put every dispatch target behind one declaration path and one registration channel, and
drew its boundary explicitly: construction strategy stays per transport. Read side by side, the
three construction machines are one shape written three times, and the differences between them do
not come from the transports.

### The same fork, three encodings

A dispatch target is built once at startup and shared by every call, or rebuilt per call against a
`ProviderContext` when it is request-scoped or a dependency elevates it. Each transport encodes that
fork in its own type:

- **HTTP** — `ControllerInstance::{Singleton(Arc<dyn Any + Send + Sync>), Request(dependency map)}`.
  The fork is expanded at startup: `__toni_routes` matches the state and instantiates one of two
  generated wrapper sets per handler — a singleton set that downcasts `Arc<dyn Any>` on every
  request, and a per-call set that rebuilds through a direct bridge call.
- **RPC** — a macro-emitted `{Singleton(Arc<Box<dyn RpcControllerTrait>>), PerCall(provider)}`
  whose `instance()` matches at call time and resolves through the provider.
- **gRPC** — a macro-emitted `{Singleton(Arc<Service>), PerCall(provider)}`, with `instance()`
  inherent because the tonic wrapper delegates through UFCS at the concrete type.

The elevation scan that chooses the arm is the same loop in all three factories — the RPC and gRPC
copies differ only in the attribute their warning names. The per-call arms fire the same two
lifecycle-bridge calls. What belongs to the transport is one thing: where `instance()` is called
from — inside `Route::execute`, inside `execute_handler`, inside the tonic wrapper's delegate —
because that is where each transport hands control back.

### Three payload types, three costs

The singleton payloads differ, and each difference costs something the transport never asked for:

- `Arc<dyn Any>` (HTTP) costs a downcast on every request, and the controller object reaches its
  own instance for lifecycle hooks through another one.
- `Arc<Box<dyn RpcControllerTrait>>` (RPC) cannot be reached for lifecycle hooks at all, so the
  object carries a second, concrete `singleton` field beside the source — and building the boxed
  copy puts a `Clone` bound on the controller struct.
- `Arc<Service>` (gRPC) is the payload the other two approximate: concrete, hook-reachable by a
  match, no downcast.

The per-call arms differ once more. RPC and gRPC resolve through the target's own provider, so a
target asked for twice in one call is built once from the execution's cache; HTTP's per-call arm
calls the construction bridge directly and is the one dispatch target absent from its own
execution's cache.

### Enhancer tokens resolve at two moments

`add_controllers_instances` resolves HTTP route enhancer tokens while loading instances, and stores
RPC and gRPC sources with their tokens unread; `RpcControllerResolver` and `GrpcServiceResolver`
read them at bind. The split is a storage artifact, not a data constraint: global enhancers are
registered before instantiation, and the role registry is complete by the phase HTTP already reads
it in. What create lacks is a slot — the resolved bundle's consumer is an adapter that may be
attached after `create` returns, and nothing at create stores a bundle for it.

The observable difference: a misdeclared enhancer token on an HTTP controller fails `create()`, and
the same mistake on an RPC controller or gRPC service fails `bind()`. ADR-0024 ordered bind so that
a wrong application reports before a busy environment; this is the same class of wrongness
reporting a phase later than its information allows.

## Decision

### One generic source in core

```rust
pub enum DispatchSource<T> {
    /// Built at startup and shared by every call.
    Singleton(Arc<T>),
    /// The target's own provider, resolved inside the call being served.
    PerCall(Arc<Box<dyn Provider>>),
}
```

`instance(&self, ctx: ProviderContext) -> Arc<T>` is the one resolution path: hand back the shared
instance, or resolve through the provider — cached in the execution — downcast to `T`, and fire
init/bootstrap through the lifecycle bridge. The caller supplies its transport's `ProviderContext`
variant; nothing else in the resolution is per transport.

The payload is gRPC's: `Arc<T>`, concrete. Lifecycle hooks reach the instance the same way on every
transport, the per-request downcast goes, and RPC's side-carried `singleton` field goes with its
`Clone` bound. Where a consumer needs erasure it coerces at its own boundary —
`RpcControllerSource::instance` answers with `Arc<dyn RpcControllerTrait>` — and the tonic wrapper,
which needs the service itself, holds the source at the concrete type and is handed `Arc<T>`
directly.

### HTTP adopts the call-time fork

One wrapper set per handler, holding the source. `Route::execute` calls `instance()` the way the
other dispatch paths do, and the per-call arm resolves through the controller's provider — a
request-scoped HTTP controller joins the execution cache it was absent from. `ControllerInstance`
and the second wrapper set are removed.

### One elevation scan

The dependency-scope scan that chooses the arm becomes one function the three factories share. The
elevation warning keeps naming the attribute the user wrote — that is the actionable half of the
message.

### Enhancer tokens resolve at create, on every transport

`add_controllers_instances` resolves RPC and gRPC tokens where it resolves HTTP's, and the
container stores each source with its resolved bundle. Bind hands a stored bundle to its adapter
instead of resolving one. A misdeclared token fails `create()`, whatever the transport.

### The call sites stay

Where `instance()` is invoked — `Route::execute`, `execute_handler`, the tonic delegate — is where
each transport hands control back, and stays as it is. The machine is one; the moments it is asked
remain the transports' own.

## Consequences

- ADR-0017's "Construction strategy stays per transport" is superseded: the strategy is one
  machine, and only the call sites remain per transport. Its enhancer-timing consequence — "a
  separate question" — is answered here. Both spots gain pointers.
- A misdeclared enhancer token fails `create()` on every transport. It failed `bind()` for RPC
  and gRPC.
- A request-scoped HTTP controller is execution-cached like every other per-call target.
- An RPC controller struct no longer needs `Clone`.
- `ControllerInstance` is removed; `DispatchSource<T>` replaces the three per-transport encodings.
- The three macro codegen paths that each emitted a construction machine collapse into one that
  instantiates the shared type.

## Roads not taken

**One erased payload (`Arc<dyn Any>` everywhere).** Uniformity by erasure keeps the per-call
downcast and adds one to every singleton call, and gRPC cannot take it at all — the tonic wrapper
needs the service at its concrete type. The concrete payload is uniform and free.

**A trait-object source.** `Arc<dyn DispatchSourceTrait>` would let core hold every source
uniformly, but an object-safe `instance()` cannot answer with `Arc<T>`, and the tonic wrapper needs
exactly that. The generic type gives each consumer the return it needs; erasure happens per
consumer, at the boundary that wants it.

**Unifying the call sites.** A common loop owning "resolve, then call" would have to absorb three
transports' pipelines — the generalisation ADR-0017 declined for `Route`, one level down. The
points where a transport hands control back are the one per-transport fact this ADR
leaves alone.

**Registering with adapters at create.** Moving registration itself, not only token resolution,
would close the timing split another way — but adapters attach after `create` returns, and
registration stays inside bind, where ADR-0024 placed it. Resolution moves to where its data is;
registration stays where its consumer is.
