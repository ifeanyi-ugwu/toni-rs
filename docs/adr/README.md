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
- [0007 — The global middleware chain anchors before routing, per adapter, pinned by a conformance suite](0007-pre-routing-global-chain.md)
- [0008 — The root module is any `ModuleMetadata`; the `ModuleDefinition` enum is removed](0008-root-module-is-any-modulemetadata.md)
- [0009 — Adapter SPI names: function first; a shared name requires shared semantics](0009-function-first-spi-naming.md)
- [0010 — Default logging: installed on create with `try_init` back-off, no runtime knob](0010-default-logger-try-init-backoff.md)
- [0011 — Trailing slashes are insignificant: canonicalized at registration and at the chain entry](0011-trailing-slash-insensitive-matching.md)
- [0012 — `{param}` is the canonical route parameter syntax; `:param` is a compile error](0012-canonical-param-syntax.md)
- [0013 — Where to listen is a value: `BindTarget` carries an address or an existing listener](0013-bind-target-listener-adoption.md)
- [0014 — A separate-port WebSocket listener is keyed by the port the gateway declares](0014-ws-listener-keyed-by-declared-port.md)
- [0015 — Handler parameters are extracted from the handler's context](0015-handler-parameters-are-from-context.md)
- [0016 — A context spans one execution and is a shared handle](0016-context-spans-one-execution.md)
- [0017 — A controller is a dispatch target, whatever the transport](0017-controller-is-a-dispatch-target.md)
- [0018 — A WebSocket connection is a session, and a session is a store](0018-a-connection-is-a-session.md)
- [0019 — A WebSocket client owns its session; an execution owns its bag](0019-a-client-owns-its-session.md)
- [0020 — Declared metadata reaches every transport, and wire fields are headers](0020-declared-metadata-and-wire-headers.md)
- [0021 — Cancellation signals the tail, because the handler is already covered](0021-cancellation-signals-the-tail.md)
- [0022 — An execution need not have a transport](0022-an-execution-without-a-transport.md)
- [0023 — Extraction is one trait, and body-freedom is a convention](0023-extraction-is-one-trait.md)
- [0024 — An application binds every transport it declares, or none](0024-bind-every-declared-transport-or-none.md)
- [0025 — An API is infallible only where its call site cannot hold an error](0025-infallible-only-where-the-call-site-cannot-hold-an-error.md)
- [0026 — Connectivity is verified by an explicit check, not by the driver's own dial](0026-startup-checks-are-explicit-and-uniform.md)
- [0027 — Extraction is the seam a pipe was reaching for](0027-extraction-is-the-pipe-seam.md)
- [0028 — A type's DI token is its full `type_name`, produced by one function](0028-one-token-format-one-owner.md)
- [0029 — A module has one identity, and display derives from it](0029-one-module-identity.md)
- [0030 — One source builds every dispatch target, and enhancer tokens resolve at create](0030-one-source-builds-every-dispatch-target.md)
