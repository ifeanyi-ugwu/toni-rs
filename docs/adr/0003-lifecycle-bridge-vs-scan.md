# 0003 — Lifecycle hooks: bridge only where the impl is invisible

Status: accepted

## Context

Lifecycle hooks (`#[on_module_init]`, `#[on_application_bootstrap]`, `#[on_module_destroy]`,
`#[before_application_shutdown]`, `#[on_application_shutdown]`) are needed on providers, controllers,
and modules. They now share one Nest-style name set and one uniform shape:
`async fn(&self[, signal]) [-> InitResult]`. Module hooks previously diverged — sync, and taking a
container argument — and were brought into line.

Providers use the [0001](0001-dispatch-not-detect-autoref-bridge.md) bridge: `#[injectable]` is on the
struct and can't see the hook methods on the `impl`, so it dispatches through inherent forwarders.

The open question was whether controllers and modules should also go through the bridge, for the sake
of "one mechanism everywhere." Doing so would have meant forwarders, the blanket trait, and an
expansion-order arrangement to keep the outer macro from stripping the now-real inner hook attributes.

## Decision

Use the bridge **only** where the macro cannot see the impl. Controllers (`#[controller]`) and modules
(`#[module]`) are attributes on the impl block — they already parse the whole impl — so they keep a
direct **scan**: find the hook methods, emit delegations.

The bridge exists solely to work around not-seeing-the-impl. Where that constraint is absent, the
bridge is machinery for its own sake: more generated code and a subtler mechanism to maintain, buying
nothing. Uniformity of *naming and shape* is the user-visible contract and is preserved across all
three; uniformity of *implementation mechanism* is not a goal in itself.

The two coexist under one name without collision because of expansion order: `#[module]` /
`#[controller]` (the outer attribute on the impl) expand first and strip the inner `#[on_module_init]`
before it would expand as the now-real bridge macro. So on a module the scan consumes the attribute;
on a bare `#[injectable]` struct the bridge macro fires. Proven by
`integration-tests/.../lifecycle_hooks.rs::startup_hooks_fire_in_order`, which asserts the exact order
across a module and its provider.

## Consequences

**Good.** Each macro uses the simplest mechanism its position allows. The scan-based paths
(`toni-macros/src/shared/lifecycle_hooks.rs` + each macro's `instance_injection.rs`) stay
straightforward; the bridge is confined to the one path that needs it. Users see one naming + shape
everywhere regardless.

**The guardrail.** A future instinct to "unify everything onto the bridge" should be resisted unless a
new constraint makes the scan insufficient — this ADR records that the divergence is deliberate, not an
oversight. Match the mechanism to whether the impl is visible, not to a wish for symmetry.

**Cost of the convergence itself.** Bringing module hooks to async/no-container was a breaking change
to the `ModuleMetadata` trait and its call sites (scanner, application context). Accepted: it removes
the lone outlier so the contract is genuinely uniform.
