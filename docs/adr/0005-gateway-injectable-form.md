# 0005 — Gateways declared like injectables: `#[websocket_gateway]` on the struct, `#[subscriptions]` on the impl

Status: accepted

## Context

[0004](0004-controller-injectable-form.md) moved controllers to the injectable form: `#[controller]`
on the struct, `#[routes]` on the impl, meeting at the concrete type through inherent-fn bridges
([0001](0001-dispatch-not-detect-autoref-bridge.md)). Gateways still sat on the **impl**, with the
struct restated inside the attribute: `#[websocket_gateway("/p", pub struct Foo { … })]`.

A gateway is a provider with a role — in NestJS terms an `@Injectable()` that implements a gateway
contract — so it should declare like one. The same structural obstacle as controllers applies: the
`#[subscribe_message]` handlers are a **variable** set of methods on the impl, which a struct attribute
cannot see and the one-slot `#[new]` bridge cannot aggregate.

Gateways carry an extra weight controllers do not: `GatewayTrait` is a **rich** trait. Beyond message
routing it has connection hooks, namespace/port, and — before this work — a dozen enhancer-token
accessor methods. Delegating all of that to a per-method bridge would mean a bridge with ~14 methods.
So the enhancer surface was collapsed first: the accessors became a single `enhancers() ->
GatewayEnhancers` descriptor the resolver reads once. That de-enrichment is the prerequisite that keeps
the behavior bridge here at five methods.

## Decision

A gateway is declared exactly like an injectable, plus one marker on its impl:

```rust
#[websocket_gateway("/chat", namespace = "lobby")]   // STRUCT — field injection, #[new]/lifecycle bridges
pub struct ChatGateway {
    #[inject] broadcast: BroadcastService,
}

#[subscriptions]                                      // IMPL — scans the handlers and hooks
#[use_guards(WsAuthGuard)]
impl ChatGateway {
    #[subscribe_message("message")]
    async fn on_message(&self, client: WsClient, msg: WsMessage) -> WsHandlerResult { /* … */ }
}
```

- `#[websocket_gateway("/p", namespace = …, port = …)]` is a **struct** attribute and produces a
  *complete* gateway on its own. It re-emits the struct with `Clone`/`InjectFields`, emits the provider
  wiring carrying the gateway **role** (so the resolver discovers it), and emits `impl GatewayTrait` with
  identity/path/namespace/port baked from the attribute. Each behavior method (`after_init`,
  `on_connect`, `on_disconnect`, `handle_event`, `enhancers`) delegates to `Self::__toni_ws_*` at the
  concrete type, resolving to the `__ws::WsHandlersBridge` default unless a `#[subscriptions]` impl
  (`handle_event` / `enhancers`) or a connection-hook macro (`on_connect` / `on_disconnect` /
  `after_init`) shadows it. Construction and lifecycle reuse the provider bridges (`__construct` /
  `__lifecycle`), so `#[inject]` fields, `#[new]`, and `#[on_*]` hooks behave as on any injectable.
- `#[subscriptions]` is an **impl** attribute and is *purely additive*: it scans the
  `#[subscribe_message]` handlers into the `handle_event` match and the gateway- and handler-level
  enhancer attrs into the `enhancers` descriptor, emitting those two inherent `__toni_ws_*` fns
  (dispatch-not-detect, the same pattern as `#[new]`). It leaves `#[new]`, `#[on_*]`, and the
  connection-hook attrs intact for their own macros.
- `#[on_connect]` / `#[on_disconnect]` / `#[after_init]` are **single-slot** connection hooks, so
  each is its own per-method macro emitting one `__toni_ws_*` forwarder — exactly like `#[new]` and
  `#[on_module_init]`. A hook stands alone (no `#[subscriptions]` impl required), and declaring one
  twice is a duplicate-definition compile error instead of a silent last-wins.

The split between `#[subscriptions]` and the hook macros follows one rule: a **variable aggregate**
(the N message handlers, the enhancer set) needs the impl-scan marker, because a struct attribute
and a one-slot bridge can't enumerate it; a **single-slot** hook is its own per-method bridge macro.
That keeps `#[subscriptions]` honest — it means subscriptions — and lets connection hooks compose
freely, including on a gateway that routes no messages at all.

Unlike the controller `RoutesBridge` (a non-async, empty default), `WsHandlersBridge` defaults are
**behavioral**: `on_connect` allows the connection, `handle_event` returns `EventNotFound`, the rest
are no-ops. So a gateway with no `#[subscriptions]` impl is a valid **connection-only** gateway — it
accepts connections (and can be injected elsewhere to broadcast) and routes nothing.

The marker on the impl is **chosen over `inventory`** for the same reasons as [0004](0004-controller-injectable-form.md):
a module-scoped framework is better served by an explicit, zero-dependency impl scan than by link-time
registration.

## Consequences

**Good.** Gateways, controllers, and providers are declared the same way. Injecting into a gateway is
`#[inject]` field or `#[new]`, identical to a provider.

**A gateway needs nothing but `#[websocket_gateway]`.** The attribute alone builds, registers, binds its
path, and accepts connections via the bridge defaults; `#[subscriptions]` only adds routing and hooks.

**Breaking change.** The inline-struct form is removed — construction is `#[new]` or field injection.
Migration is mechanical (lift the struct out of the attribute, add `#[subscriptions]`, tag a real
constructor with `#[new]`) and was done across the codebase when this landed.

**Deferred.** `#[rpc_controller]` keeps its current form; the same de-enrichment-then-struct-attr
treatment (`#[patterns]` on the impl) applies to it next.
