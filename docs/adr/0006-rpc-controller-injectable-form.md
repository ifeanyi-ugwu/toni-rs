# 0006 — RPC controllers declared like injectables: `#[rpc_controller]` on the struct, `#[patterns]` on the impl

Status: accepted

## Context

[0004](0004-controller-injectable-form.md) and [0005](0005-gateway-injectable-form.md) moved HTTP
controllers and WebSocket gateways to the injectable form — the role attribute on the struct, an
impl-marker that scans handlers, meeting at the concrete type through inherent-fn bridges
([0001](0001-dispatch-not-detect-autoref-bridge.md)). RPC controllers were the last holdout:
`#[rpc_controller]` sat on the impl, with the struct restated inside the attribute
(`#[rpc_controller(pub struct Foo { … })]`).

An RPC controller is a provider with a role, the same as a gateway. Its `#[message_pattern]` /
`#[event_pattern]` handlers are a variable set on the impl — which a struct attribute can't see and
the one-slot `#[new]` bridge can't aggregate — so the impl needs a marker. As with the gateway, the
trait's enhancer surface was collapsed first (a single `enhancers() -> RpcEnhancers` descriptor in
place of nine accessors) so the bridge stays small.

## Decision

An RPC controller is declared exactly like an injectable, plus one marker on its impl:

```rust
#[rpc_controller]                          // STRUCT — field injection, #[new]/lifecycle bridges
pub struct OrdersController {
    #[inject] repo: OrdersRepo,
}

#[patterns]                                // IMPL — scans the pattern handlers
impl OrdersController {
    #[message_pattern("order.create")]
    async fn create(&self, data: CreateOrder, ctx: &RpcContext) -> Result<RpcData, RpcError> { /* … */ }

    #[event_pattern("order.cancelled")]
    async fn on_cancelled(&self, data: OrderId, ctx: &RpcContext) -> Result<(), RpcError> { /* … */ }
}
```

- `#[rpc_controller]` is a **struct** attribute and produces a *complete* controller on its own: it
  re-emits the struct with `Clone`/`InjectFields`, emits the provider wiring carrying the
  rpc-controller **role**, and emits `impl RpcControllerTrait` with `get_token` baked from the struct
  name. `get_patterns`, `handle_message`, and `enhancers` delegate to `Self::__toni_rpc_*`, resolving
  to the `__rpc::RpcHandlersBridge` default when no `#[patterns]` impl shadows them. Construction and
  lifecycle reuse the provider bridges, so `#[inject]`, `#[new]`, and `#[on_*]` behave as on any
  injectable.
- `#[patterns]` is an **impl** attribute and is *purely additive*: it scans the `#[message_pattern]` /
  `#[event_pattern]` handlers into the `handle_message` match and the pattern list, and the enhancer
  attrs into the `enhancers` descriptor, emitting the three inherent `__toni_rpc_*` fns. It leaves
  `#[new]` and `#[on_*]` intact for their own macros.

Unlike the gateway, **RPC has no connection hooks** (no per-connection lifecycle — every message is a
request/response or a fire-and-forget event). So there are no single-slot hook macros to split out:
`#[patterns]` is *pure aggregation*, the same shape as `#[routes]`. And unlike the gateway's path /
namespace / port — which are baked from the attribute because the struct knows them — the RPC
**pattern list is impl-derived**, so `get_patterns` bridges through `#[patterns]` like the handlers do.

A controller with no `#[patterns]` impl is valid: it registers as a provider but exposes no patterns
(the bridge defaults answer — empty pattern list, `PatternNotFound`, no enhancers).

## Consequences

**Good.** HTTP controllers, gateways, RPC controllers, and providers are now declared the same way.
Injecting into an RPC controller is `#[inject]` field or `#[new]`, identical to a provider. The
struct-in-attribute form is gone across all three roles.

**Breaking change.** The inline-struct form is removed — construction is `#[new]` or field injection.
Migration is mechanical (lift the struct out of the attribute, add `#[patterns]`, tag a real
constructor with `#[new]`) and was done across the codebase when this landed.

This completes the injectable-form rollout begun in [0004](0004-controller-injectable-form.md).
