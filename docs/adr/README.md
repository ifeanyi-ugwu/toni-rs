# Architecture Decision Records

Records of architectural decisions with lasting consequences and non-obvious rationale — the *why*
behind choices that a reader of the code alone would have to reverse-engineer or, worse, would
unknowingly undo.

An ADR is warranted when a decision (a) shapes how a whole class of features is built, (b) rests on a
constraint or trade-off that isn't visible from the resulting code, or (c) records a road deliberately
*not* taken. Routine implementation choices don't need one; `git log` and code comments cover those.

## Format

Lightweight [MADR](https://adr.github.io/madr/): context → decision → consequences. Numbered,
append-only. A superseded ADR stays in place with its status changed and a pointer to the successor —
the history is part of the value.

## Index

- [0001 — Dispatch, don't detect: the autoref bridge for macros that can't see the impl](0001-dispatch-not-detect-autoref-bridge.md)
- [0002 — One `#[injectable]` provider form; enhancer roles detected from trait impls](0002-one-injectable-form-marker-free-roles.md)
- [0003 — Lifecycle hooks: bridge only where the impl is invisible](0003-lifecycle-bridge-vs-scan.md)
- [0004 — Controllers declared like injectables: `#[controller]` on the struct, `#[routes]` on the impl](0004-controller-injectable-form.md)
- [0005 — Gateways declared like injectables: `#[websocket_gateway]` on the struct, `#[subscriptions]` on the impl](0005-gateway-injectable-form.md)
- [0006 — RPC controllers declared like injectables: `#[rpc_controller]` on the struct, `#[patterns]` on the impl](0006-rpc-controller-injectable-form.md)
- [0008 — The root module is any `ModuleMetadata`; the `ModuleDefinition` enum is removed](0008-root-module-is-any-modulemetadata.md)
