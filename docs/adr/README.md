# Architecture Decision Records

Records of architectural decisions with lasting consequences and non-obvious rationale — the *why*
behind choices that a reader of the code alone would have to reverse-engineer or, worse, would
unknowingly undo.

An ADR is warranted when a decision (a) shapes how a whole class of features is built, (b) rests on a
constraint or trade-off that isn't visible from the resulting code, or (c) records a road deliberately
*not* taken. Routine implementation choices don't need one; `git log` and code comments cover those.

## Format

Lightweight [MADR](https://adr.github.io/madr/): context → decision → consequences. Numbered, and
never renumbered or removed. A superseded ADR stays in place with its status changed and a pointer
to the successor — the history is part of the value.

## After acceptance

An accepted record is not rewritten to track the code. Its own text takes two edits: a successor's
pointer, and the correction of a clause that was wrong when written. What changed decides which:

- **The decision.** A later ADR that refines the whole decision adds a pointer under its title,
  above the body, so a reader meets it before the older text: 0048 carries one to 0049, which moved
  the boundary 0048 drew. One that supersedes a section adds the pointer at that section, and one
  that revisits a consequence adds it where the older record draws the consequence. One that
  supersedes the record changes its status to name the successor.
- **A name.** A type, variant, method or term renamed since is left as written, and the change that
  renamed it carries the mapping. Editing the prose to follow a rename turns a record of a decision
  into a description of the present.
- **A clause that was wrong when written**, such as a claim about code that did not yet exist, is
  corrected in place by the change that finds it.

A rename that touches every record's paths, as the framework's own did, is recorded as an ADR of its
own (0044) and applied across the records in the same pull request, so they stay greppable against
the code.

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
- [0031 — One attribute declares a controller, and its handlers name the transport](0031-one-controller-attribute.md)
- [0032 — An RPC reply can be a stream, and the execution rides it](0032-a-reply-can-be-a-stream.md)
- [0033 — A gRPC streaming reply is part of its execution](0033-a-grpc-streaming-reply-is-part-of-its-execution.md)
- [0034 — An integration earns a module when it owns a lifetime](0034-an-integration-earns-a-module-when-it-owns-a-lifetime.md)
- [0035 — Observing errors is a pattern, not a pipeline role](0035-observing-errors-is-a-pattern.md)
- [0036 — A refused WebSocket connection is told why](0036-a-refused-connection-is-told-why.md)
- [0037 — A gRPC handler hands the chain its error, not the status](0037-a-grpc-handler-hands-the-chain-its-error.md)
- [0038 — A gRPC handler is written in ulo's shapes, and the macro writes tonic's](0038-a-grpc-handler-is-written-in-ulos-shapes.md)
- [0039 — A gRPC status carries the error it came from](0039-a-grpc-status-carries-the-error-it-came-from.md)
- [0040 — A name the framework reads is backed by a type it checks](0040-a-name-the-framework-reads-is-backed-by-a-type.md)
- [0041 — An RPC handler takes what every other handler takes](0041-an-rpc-handler-takes-what-every-handler-takes.md)
- [0042 — A gRPC handler asks the type what the wire carries](0042-a-grpc-handler-asks-the-type-what-the-wire-carries.md)
- [0043 — A gRPC method's shape comes from the proto](0043-a-grpc-method-s-shape-comes-from-the-proto.md)
- [0044 — The framework is named ulo](0044-the-framework-is-named-ulo.md)
- [0045 — A public error names what a caller can act on](0045-a-public-error-names-what-a-caller-can-act-on.md)
- [0046 — The unit a scope names is an execution](0046-the-unit-a-scope-names-is-an-execution.md)
- [0047 — The context family names the execution](0047-the-context-family-names-the-execution.md)
- [0048 — A gRPC reply travels through the pipeline](0048-a-grpc-reply-travels-through-the-pipeline.md)
- [0049 — An answer is an envelope and a cardinality](0049-an-answer-is-an-envelope-and-a-cardinality.md)
- [0050 — What a handler may return is a type](0050-what-a-handler-may-return-is-a-type.md)
- [0051 — An application is Send](0051-an-application-is-send.md)
- [0055 — The pipeline's order is the order that was written](0055-the-pipelines-order.md)
- [0056 — One role per position: the pre-dispatch role reaches every transport](0056-the-pre-dispatch-role.md)
