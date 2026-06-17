# 0004 — Controllers declared like injectables: `#[controller]` on the struct, `#[routes]` on the impl

Status: accepted

## Context

[0002](0002-one-injectable-form-marker-free-roles.md) gave providers a "plain struct, one attribute"
form: `#[injectable]` on the struct, `#[inject]` fields, `#[new]` constructor, lifecycle hooks — all
reached through the `toni::__construct` / `toni::__lifecycle` bridges ([0001](0001-dispatch-not-detect-autoref-bridge.md)).

Controllers never got that form. `#[controller]` sat on the **impl**, so field injection required
restating the struct inside the attribute: `#[controller("/p", pub struct Foo { #[inject] dep: Dep })]`.
The struct lived half in an attribute argument and half nowhere, deviating from `#[injectable]` for no
reason a user could see.

The obstacle is structural: route handlers are a **variable** set of methods living on the impl. A
struct attribute cannot enumerate methods it cannot see, and the `#[new]`-style inherent-fn bridge is a
**one-slot** mechanism — it dispatches a single fixed call, so it cannot aggregate N arbitrarily-named
routes. Aggregating a variable set needs either a macro that sees the whole impl at once, or a
link-time registry (`inventory`/`ctor`).

## Decision

A controller is declared exactly like an injectable, plus one marker on its impl:

```rust
#[controller("/users")]              // on the STRUCT — field injection, Clone, #[new]/lifecycle bridges
pub struct UsersController {
    #[inject] svc: UserService,
}

#[routes]                            // on the IMPL — scans the handlers
impl UsersController {
    #[new] fn new(svc: UserService) -> Self { /* optional, as with #[injectable] */ }
    #[get("/")] async fn list(&self) -> impl IntoResponse { /* … */ }
}
```

- `#[controller("/p", scope = "…")]` is a **struct** attribute. It mirrors the `#[injectable]`-from-struct
  path: it re-emits the struct with `Clone`/`InjectFields` and emits inherent bridge fns —
  `__toni_build_from_deps` (field injection or the `#[new]` constructor, via the `__construct` bridge),
  `__toni_dependencies`, `__toni_prefix`, `__toni_is_request_scoped`. Construction and lifecycle reuse
  the same bridges providers use.
- `#[routes]` is an **impl** attribute. It scans the `#[get]`/`#[post]`/… handlers and emits the per-route
  `Route` wrappers, the `Controller` object, and the `ControllerFactory` — all delegating dependency
  resolution, the route prefix, and scope to the struct's bridges. The full path is `__toni_prefix()`
  joined with each handler's sub-path at registration time.

The marker on the impl is **chosen over `inventory`** (the only marker-free alternative). For a
module-scoped DI framework, an explicit, zero-dependency, debuggable impl scan is worth one attribute;
`inventory` brings link-time registration, a portability hit (not wasm-friendly), and a process-global
route table that fights the module model. The struct — the part a user reads — is pure injectable either
way.

## Consequences

**Good.** Controllers and providers are declared the same way. "How do I inject into a controller?" is
answered by "`#[inject]` field or `#[new]`," identical to a provider. The struct-in-attribute form is
gone.

**Failures move to the use site, loudly.** A `#[controller]` struct with no `#[routes]` impl fails to
compile where the factory calls `Self::__toni_build_from_deps` ("no associated function"); a `#[routes]`
impl with no `#[controller]` struct fails the same way. Neither silently does nothing.

**Breaking change.** The inline-struct form, bare-`new()` auto-detection, and `init = "…"` are removed —
construction is `#[new]` or field injection, as with `#[injectable]`. Migration is mechanical (lift the
struct out of the attribute, add `#[routes]`, tag a real constructor with `#[new]`) and was done across
the codebase when this landed.

**Deferred.** `#[websocket_gateway]` and `#[rpc_controller]` keep their current form; the same
struct-attr + impl-marker treatment applies to them later.
