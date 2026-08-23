# 0022 — An execution need not have a transport

Status: accepted

## Context

ADR-0016 defined an execution as the span of one unit of work — one HTTP request, one RPC call, one
WebSocket message — and moved the instance cache onto the context so every transport could carry one.
Request scope stopped being an HTTP privilege at that point. Reaching an execution by hand did not.

### Resolving by hand required an HTTP request that meant nothing

`ToniApplicationContext::resolve` took `&RequestPart` and built an `HttpContext` from it. A CLI tool,
a cron tick or a test that wanted a request-scoped provider therefore wrote:

```rust
let parts = http::Request::builder().body(()).unwrap().into_parts().0;
let service = ctx.resolve::<RequestService>(&parts).await?;
```

The request is a prop. Nothing reads its method, its URI or its headers; it exists because the only
way to obtain an execution was to name a transport, and HTTP was the one with a constructor that
needs no connection behind it.

The manufacture was also invisible at the call site. Two `resolve` calls that read as one unit of work
produced two executions and two instances of a request-scoped provider, which the doc comment had to
warn about because the signature could not.

### The absence of an execution is a different thing from an execution with no wire

`ProviderContext::None` means no execution: module initialisation, `ApplicationContext::get`. A
request-scoped provider is refused there, because there is no cache to build it into and nowhere for
its lifetime to end.

That left two states and three needs. A caller outside any transport had no way to say "this is one
unit of work" other than by fabricating a transport, and no way to be refused for the right reason.

### Nothing an execution is made of comes from a transport

`HandlerContext` requires four things, and every concrete context supplies all four from one shared
struct:

```rust
pub(crate) struct SharedState {
    pub(crate) metadata: Option<Arc<Metadata>>,
    pub(crate) extensions: Extensions,
    pub(crate) cancellation: CancellationToken,
    pub(crate) cache: ExecutionCache,
}
```

Declared metadata, an extension bag, a cancellation token, an instance cache. None of them is wire
state. The transport contributes the fields *beside* `SharedState` — request parts, a pattern, a
client — and nothing an execution needs in order to be one.

## Decision

### An execution with no transport is a first-class context

`StandaloneContext` holds a `SharedState` and implements `HandlerContext` by delegating to it, the
same way the four transport contexts do. `ProviderContext::Standalone` carries it, and
`ProviderContext::standalone()` builds one.

Every answer is honest, which is the bar `HandlerContext` sets. Metadata is `None` — nothing declared
any, the same answer a global error handler gives. The bag, the cache and the cancellation token are
its own; a caller holding the execution can cancel it.

`ProviderContext::None` keeps its meaning. No execution at all, and a request-scoped provider is
still refused there. The distinction between the two is exactly what that refusal reads.

### Resolution takes the execution rather than inventing one

One verb, on both the application context and `ModuleRef`:

```rust
resolve::<T>(&execution)
resolve_by_token::<T>(token, &execution)
```

The caller decides what a unit of work is by deciding how long to hold the execution. Two resolutions
against one execution share a request-scoped instance; two executions build two. Neither fact needs a
doc comment to be visible, because the argument says which is happening.

The `&RequestPart` form is removed rather than kept as a shorthand. It was the last resolution entry
point that could only be reached through HTTP, and its behaviour — a fresh execution per call — is
the kind that should be readable at the call site.

### The enum becomes `#[non_exhaustive]`

Adding a variant breaks every exhaustive match on it. This release adds one; `#[non_exhaustive]`
means the next does not, and matching on a specific variant, which is what the framework and its
integrations actually do, is unaffected.

### A standalone execution is state, not a dispatch

Nothing runs an enhancer chain over it. It carries what an execution carries so that providers can be
built into it; it does not make the caller a handler. A guard could be written over
`StandaloneContext` — a blanket impl over `C: HandlerContext` already covers it — and nothing would
ever call one.

## Consequences

- `resolve` and `resolve_by_token` change signature on `ToniApplicationContext` and `ToniApplication`.
  Breaking for callers that passed request parts; the fix is `HttpContext::from_parts(parts).into()`
  where the execution is genuinely an HTTP one, and `ProviderContext::standalone()` where it never was.
- `ModuleRef` can reach a request-scoped provider, which it could not before.
- A CLI tool, worker or test resolves without constructing an HTTP request it does not use.
- A request-scoped instance in a standalone execution lives as long as the caller holds the execution.
  That is the caller's decision to make and their mistake to make.
- `Request`, the built-in provider over HTTP request parts, still panics in a standalone execution. It
  does the same on the other three transports: there is no request to read.
- Provider instances are not scoped to anything new. `Standalone` is a fifth way to spell "here is an
  execution", not a fifth scope.

## Roads not taken

**Keeping `resolve(&parts)` as a shorthand.** Two names for one operation, one of which quietly
manufactures state and only works for one transport. The ceremony it saved was one line.

**Naming it `Test` or `Synthetic`.** A cron tick and a CLI command are executions in production. A
name that implies otherwise would make the honest use look like a misuse.

**Letting `ProviderContext::None` build a cache on demand.** It would make request scope resolvable
during module initialisation, where an instance would have no end to its life. The refusal is the
feature.

**An ambient current execution.** ADR-0016 declined a thread-local or task-local execution on the
ground that nothing in it is checkable; a standalone execution is a value the caller passes, which is
the opposite property.
