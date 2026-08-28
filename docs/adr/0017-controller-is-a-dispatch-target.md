# 0017 — A controller is a dispatch target, whatever the transport

Status: accepted

## Context

ADR-0016 made an RPC controller build per call and stop being injectable, and the same reasoning was
applied to gRPC services. Both are declared in `providers:`, and neither is a provider any more.

### `providers:` holds things the framework refuses to provide

A module lists an RPC controller under `providers:` and the injector then refuses to resolve it. The
refusal is deliberate and correct — the scope a dispatch target is built at follows its own
dependencies, so a holder could not know whether it held one instance or one per call — but the
declaration states a property the framework denies. Nothing in the module declaration distinguishes
the entries that resolve from the entries that do not.

The distinction exists, only at runtime: `dispatch_target_tokens` on the container, `dispatch_targets`
on the module, and an interception in `resolve_dependencies` that runs before the ordinary lookup.

### The divergence had a justification, and it expired

Filing `#[rpc_controller]` under `providers:` was a deliberate divergence. Nest keeps `controllers`
and `providers` as separate collections and resolves only the latter, so a Nest microservice
controller cannot be injected; toni's could. That bought a property, which is why the divergence was
recorded as possibly right rather than wrong.

ADR-0016 removed the property. What remains is the divergence with nothing behind it.

### The same structure exists twice, and the code says so

`Module::dispatch_targets` carries this comment:

> Instances reached only by dispatch — RPC controllers and gRPC services. Kept for lifecycle hooks
> the way `controller_objects` is, and deliberately absent from `providers_instances` so nothing
> resolves one as a dependency.

`controller_objects` is the HTTP controller collection, kept for lifecycle hooks and absent from
dependency resolution. Two collections hold the same kind of thing for the same reason, because HTTP
controllers arrive through `controllers:` and the others arrive through `providers:`.

### The controller path already does what the other transports need

`create_instances_of_controllers` resolves the declared dependency tokens, calls
`ControllerFactory::build`, expands the built object into its dispatch units, resolves each unit's
declared enhancer tokens against the role registry, and stores the object for lifecycle and the units
for dispatch. `RpcControllerResolver` and `GrpcServiceResolver` are that shape written twice more.

Request-scope elevation is already there too: `ControllerFactory::build` inspects each resolved
dependency's `get_scope()` and elevates the controller when one is request-scoped. The RPC and gRPC
factories carry a copy of that scan.

What the controller path lacks is a channel. `ControllerFactory::build` returns `Arc<dyn Controller>`,
and a `Controller` can hand back only `Vec<Arc<dyn Route>>` — an HTTP route list. A transport with a
different dispatch unit has nowhere to put it, which is why dispatch registration travels through
`ProviderRole` instead.

### Where the transports differ

Only in the dispatch unit: a route keyed by path and method, a set of patterns, a tonic service
registration. Token, dependencies, lifecycle, elevation and enhancer-token resolution are common.

## Decision

### `controllers:` holds dispatch targets; `providers:` holds what can be held

Resolvability is the criterion. Anything something else may hold is a provider; anything reached only
by its transport's dispatch is a controller, whatever that transport calls it.

`#[controller]`, `#[rpc_controller]` and `#[grpc_service]` are declared in `controllers:`.
`#[websocket_gateway]` stays in `providers:` — a gateway is held, to broadcast from elsewhere — and it
stays there *because* of the criterion rather than by inheritance from Nest.

### A controller hands over a `Dispatch`, not routes

```rust
pub trait Controller: Send + Sync {
    fn get_token(&self) -> String;
    fn dispatch(&self) -> Dispatch;
    // lifecycle hooks unchanged
}

pub enum Dispatch {
    Http(Vec<Arc<dyn Route>>),
    Rpc(Arc<dyn RpcControllerSource>),
    Grpc(Arc<dyn GrpcServiceSource>),
}
```

`Route` is unchanged. It is HTTP's dispatch unit — path, method, per-route enhancers — and
generalising it would push HTTP's shape onto transports that do not have one. The agnosticism sits one
level above it.

### The variance point is a closed enum

`ProviderRole`, `ProviderContext` and `BoundAdapters` are already closed enums keyed by transport, and
adding a transport already means adding a variant to each. `Dispatch` joins them.

A registrar callback — `fn register(&self, &mut dyn DispatchRegistrar)` — would let an out-of-tree
transport declare dispatch targets. That is not a capability this framework offers, and inverting
control would cost the loader its say over the order registration happens in.

### Dispatch stops travelling through the provider role channel

`ProviderRole::RpcController` and `ProviderRole::GrpcService` are removed. So are
`Module::dispatch_targets` and `ToniContainer::dispatch_target_tokens`: a dispatch target declared in
`controllers:` is absent from `providers_instances` structurally, exactly as an HTTP controller is, and
is kept for lifecycle in `controller_objects` alongside one.

### Injecting a dispatch target fails as not-found

The named refusal is removed with the token set that made it possible. A dispatch target is no longer
in the provider store to intercept, so the failure is the ordinary missing-dependency path.

This is accepted rather than regretted: the special-cased message existed only because the token was
somewhere it should not have been. Keeping it would mean the container tracking controller tokens for
the error path alone, re-introducing a cross-reference between the two lists to buy a message.

The weaker message only covers the weaker mistake. Declaring a dispatch target in `providers:` does
not compile at all — the macro emits no provider factory for one, so the list has nothing to name.
Only reaching for one through `#[inject]` reaches runtime, and there the token is absent the way any
undeclared provider's is.

### Construction strategy stays per transport

This unifies declaration and registration, not construction. `ControllerInstance`,
`RpcControllerSource` and `GrpcServiceSource` each encode their own singleton-or-per-call fork, and each
resolves its instance where its transport hands control back — inside `Route::execute`, inside
`execute_handler`, inside the tonic wrapper's delegate. Those points are not interchangeable, and no
common shape is imposed on them here.

Superseded by [ADR-0030](0030-one-source-builds-every-dispatch-target.md): one source type encodes
the fork on every transport, and only the call sites remain per transport.

## Consequences

- Breaking for every module declaring an RPC controller or a gRPC service: the entry moves from
  `providers:` to `controllers:`.
- `#[rpc_controller]` and `#[grpc_service]` emit a `ControllerFactory` rather than a `ProviderFactory`.
  The elevation scan and dependency resolution are already in the controller factory, so each loses its
  copy.
- Two module collections become one, and one enum variant set replaces two role variants plus a token
  set.
- Adding a transport with dispatch targets means a `Dispatch` variant, in step with the enums it
  already means a variant in.
- Enhancer-token resolution keeps its per-transport timing: HTTP resolves at init while building
  routes, RPC and gRPC resolve at bind. One declaration path does not make one resolution moment, and
  unifying that is a separate question — answered by
  [ADR-0030](0030-one-source-builds-every-dispatch-target.md): tokens resolve at create on every
  transport.
- The refusal diagnostic degrades to not-found, by decision.

## Roads not taken

**Generalising `Route` across transports.** `Route` would have to lose `get_path` and
`get_method`, or gain transport variants of both. HTTP's dispatch unit is path-and-method
keyed; the other transports' units are not, and flattening them into one type describes none of them
well.

**A third module list.** A separate key for non-HTTP dispatch targets would keep `controllers:` HTTP-only
and avoid the trait change. It also re-states the same category under two names, and leaves a reader
asking which list a new transport belongs in.

**Renaming `controllers:`.** "Dispatch target" is the accurate category name, and `controllers:` is the
familiar one. The surface stays as it is; the trait behind it carries the accurate meaning.

**A registrar callback instead of the enum.** Recorded under the decision above — rejected for the
capability it opens rather than the code it costs.
