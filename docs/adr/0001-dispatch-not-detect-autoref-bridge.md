# 0001 — Dispatch, don't detect: the autoref bridge for macros that can't see the impl

Status: accepted

## Context

Several ulo macros generate code for a type from *one* item while the information they need lives in
a *sibling* item they cannot read. `#[injectable]` sits on the struct but needs to know about the
`#[new]` constructor and the `#[on_module_init]` hooks on the `impl`. A proc-macro only ever receives
the tokens of the item it is attached to, so the struct macro is structurally blind to the impl.

The naive approach is *detection*: have the struct macro discover "does this type have a constructor?
which lifecycle hooks?" and branch on the answer. It can't — the impl isn't in its input. Reflection
over trait impls doesn't exist in Rust, and specialization is unstable.

## Decision

Don't detect — **dispatch**. The generated code unconditionally names a well-known associated item;
the type system, not the macro, decides which definition it reaches.

The mechanism, applied uniformly by `ulo/src/__construct.rs` (constructors) and
`ulo/src/__lifecycle.rs` (hooks):

1. A blanket trait provides a default for every type: `impl<T: ?Sized> LifecycleBridge for T`
   with `async fn __ulo_lc_on_init(&self) { /* no-op */ }`, and similar; `impl<T> CtorBridge for T`
   with `const __ULO_ONE_NEW_PER_TYPE: Option<Ctor<Self>> = None`.
2. The marker macro on the impl method emits an *inherent* item of the same name beside the user's:
   `#[on_module_init]` a method forwarding to it, `#[new]` a const holding fn pointers that return
   the constructor's dependency tokens and that resolve them and call it.
3. The struct macro's generated code always names that item. Where the inherent item exists (the
   user wrote the hook/constructor) it wins resolution; otherwise the blanket default applies.

The struct macro needs zero knowledge of which hooks or constructor exist. The `ulo::__detect`
enhancer probes are the same idea in a returns-`Option` shape: the probe yields `Some(coerced role)`
when the type implements the role trait and `None` otherwise, so the factory registers exactly the
roles a type actually implements.

## Consequences

**Good.** One mechanism powers `#[new]`, lifecycle hooks, and enhancer-role detection. Adding a hook
or role is a new bridge item, not new detection logic. A misuse fails at the call site (a trait not
implemented → loud compile error) rather than silently doing nothing.

**The load-bearing trap — call via UFCS on the concrete type, never method syntax.** The blanket impl
covers `Arc<Struct>`, not only `Struct`. The provider wrapper holds `instance: Arc<Struct>`, so
`self.instance.__ulo_lc_on_init()` (method syntax) binds the blanket *no-op* at the `Arc` layer and
never derefs to the inherent forwarder on `Struct` — every hook silently runs the no-op, no error.
Deref-based method lookup stops at the first type that matches, and the blanket matches one deref too
early. The fix, and the rule for any future bridge call through a smart pointer:

```rust
// WRONG — binds the blanket no-op on Arc<Struct>, hook never fires
self.instance.__ulo_lc_on_init().await
// RIGHT — UFCS pins resolution to Struct, inherent wins when present
Struct::__ulo_lc_on_init(&*self.instance).await
```

(See `generate_bridge_lifecycle_methods` in
`ulo-macros/src/provider_macro/instance_injection.rs`. `#[new]`'s const is read as
`<Struct>::__ULO_ONE_NEW_PER_TYPE`; a const has no method-call form, so the `Arc` trap cannot reach
it.)

**Second constraint — the probe/dispatch call must sit at a concrete-type site.** Inside a generic
`fn f<T>()` the bound is erased and the probe always takes the fallback. The macro therefore emits the
call inline where the struct is named, not through a generic helper.

**Cost paid for not detecting.** Every provider carries the blanket no-op machinery and the generated
code calls all five hooks unconditionally (most resolve to no-ops). This is cheap — the calls inline
to nothing — and buys the property that the struct macro never has to see the impl.

These traps are subtle and silent (a wrong call compiles and runs, doing nothing). A contributor
extending the pattern will re-hit them without this record — which is the reason this ADR exists.
