# 0031 — One attribute declares a controller, and its handlers name the transport

Status: proposed

## Context

ADR-0017 put every dispatch target in one module list; ADR-0030 built every one through one
machine. Three struct attributes remain — `#[controller]`, `#[rpc_controller]`, `#[grpc_service]`
— and after 0030 they differ only in name, in the `Dispatch` variant their generated `dispatch()`
hands over, and in which attribute their elevation warning tells the user to write.

### The attribute repeats what the handler impl already says

A controller's transport is visible in its handler impl: `#[routes]` carries HTTP verbs,
`#[patterns]` carries message patterns, `#[grpc_methods]` sits on a tonic trait impl. Declaring
`#[rpc_controller]` above a `#[patterns]` impl names RPC twice, and the struct attribute is the
copy that carries nothing else — dependencies, scope and lifecycle are declared the same way under
all three names.

### `#[grpc_service]` still carries the inline-struct form

`#[grpc_service(pub struct X {…})]` on the inherent impl kept its inline struct when every other
declaration moved to the struct-attribute form. It is the one surviving inline form, and the
attribute is the only thing keeping it alive: with a shared struct attribute there is a struct
site for the declaration to live on.

### The two sides already meet through a bridge

A struct attribute cannot see the handler impl, and the handler impl cannot see the struct. HTTP
already crosses that gap with `RoutesBridge`: a blanket empty default, shadowed by an inherent fn
the `#[routes]` impl emits, resolved at the concrete-type call site the struct macro generates.
The mechanism carries any per-transport answer, not only routes.

## Decision

### `#[controller]` is the only struct attribute for dispatch targets

`#[rpc_controller]` and `#[grpc_service]` are removed. The names stop existing, with no migration
error behind them — the crate has no users to migrate. `#[controller]` keeps its two arguments,
the HTTP route prefix and `scope = "request"`, and emits what ADR-0030 already made uniform: the
re-emitted struct, the construction bridges, and the one construction machine.

### The handler impl answers with a `Dispatch`

A `DispatchBridge` replaces `RoutesBridge`: a blanket default
`__toni_dispatch(&DispatchSource<Self>) -> Dispatch` answering `Dispatch::Http(Vec::new())`, so a
controller with no handler impl stays valid and dispatches nothing, as today. Each handler-impl
macro shadows it with an inherent fn:

- `#[routes]` answers `Dispatch::Http`, one route wrapper per handler.
- `#[patterns]` answers `Dispatch::Rpc` with the controller's source companion. The companion and
  the `RpcControllerTrait` impl move here from the struct attribute — declared where the transport
  is.
- `#[grpc_methods]` answers `Dispatch::Grpc` with the service's source companion, which it already
  emits.

### One transport per struct

Two handler impls on one struct each emit `__toni_dispatch`, and the duplicate inherent definition
fails to compile. The error is rustc's — duplicate definitions with that name — not a named
refusal; the macros cannot see each other to say more.

### A prefix on a non-HTTP controller warns at startup

The route prefix is HTTP's argument, and only `#[routes]` reads it. The `#[patterns]` and
`#[grpc_methods]` dispatch bodies check `__toni_prefix()` and warn when a controller declares a
path its transport cannot use.

### The gRPC declaration becomes ordinary

A gRPC service is a plain struct under `#[controller]`, a plain inherent impl whose `#[new]` and
`#[on_*]` attributes expand on their own, and `#[grpc_methods]` on the tonic trait impl. The
inline-struct form dies with the attribute that carried it.

## Consequences

- Breaking for every `#[rpc_controller]` and `#[grpc_service]` declaration: the attribute becomes
  `#[controller]`, and a gRPC service's struct moves out of the attribute into an ordinary
  declaration.
- The elevation warning names one attribute — `#[controller(scope = "request")]` — on every
  transport, and `scope = "request"` is declared there alone; the per-attribute copies go.
- A dispatch target's `#[inject]` fields resolve through the construction bridges `#[controller]`
  emits — one field-resolution path, whatever the transport.
- `RoutesBridge` is replaced by `DispatchBridge`.
- No declaration form remains that takes a struct inline in an attribute.

## Roads not taken

**Transport markers on the struct attribute** (`#[controller(rpc)]`). Restates what the handler
impl declares, and reopens the drift the removal closes: two places to name the transport is one
more place to be wrong.

**Hybrid controllers — several transports on one struct.** Aggregating dispatch across handler
impls needs machinery the shadowing bridge cannot give: a distributed registry, or transport
markers on the struct attribute so the struct macro knows what to collect. The service layer
already shares logic across transports, so a second thin dispatch skin over one service costs a
few lines. `Dispatch` stays a single answer; a collection can replace it if the case ever
arrives.

**Keeping the per-transport attribute names as aliases.** An alias carries no information the
handler impl does not, and two names for one declaration is a documentation surface with no
behavior behind it.
