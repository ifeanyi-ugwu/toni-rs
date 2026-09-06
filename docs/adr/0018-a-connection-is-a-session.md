# 0018 — A WebSocket connection is a session, and a session is a store

Status: accepted

## Context

ADR-0016 settled that a context spans one execution, and that a connection is a session rather than an
execution. It left the session itself undecided: "a session scope — store, lifetime, teardown, API — is
its own decision, to be recorded when something needs one."

### An execution ends, and some state has to outlive it

Everything an execution knows dies with it. `WsClient::extensions` says so:

> Scoped to one execution, not to the connection: the next message arrives with an empty bag, and
> nothing written at connect survives into it.

So the everyday WebSocket shape — authenticate once at connect, act on that identity for every later
message — has nowhere to live. The connect execution now reaches its own hook, which is what makes the
missing half visible: a connect guard's work is available to `on_connect` and to nothing after it.

### A gateway cannot hold connection state, and should not

One gateway serves every connection on its path; the wrapper holds one gateway and a map of many
clients. It is a singleton, and a request-scoped dependency on it is refused at startup. A
per-connection value injected into it would belong to whichever connection built it first, for the
life of the process.

The refusal is therefore the right answer rather than an obstacle, and it rules out the gateway as the
place connection state could live.

### Nest reaches connection scope through which object it passes

`packages/websockets/web-sockets-controller.ts` calls the same `getContextId` the HTTP router calls,
and stamps the id on the object it is given:

```ts
const contextId = ContextIdFactory.getByRequest(request);
if (!request[REQUEST_CONTEXT_ID]) {
  Object.defineProperty(request, REQUEST_CONTEXT_ID, { value: contextId, ... });
}
```

The HTTP router passes the request; the websocket controller passes the **client**. A request object is
new each time and a client lives for the connection, so the same code yields per-request scope on one
transport and per-connection scope on the other. No comment in that file explains the choice.

The result is a scope doing two jobs: Nest has connection-lifetime state on WebSocket and no
per-message scope at all.

## Decision

### A connection gets a store, not a context

The session carries a bag and nothing else — no cache, no cancellation, no route metadata, no answer,
no phases. A context is the object for one execution; a session is keyed state with a longer lifetime.

Modelling it as a second context type would put two context-shaped things on one transport, and the
confusion between them would be structural rather than merely possible.

### It is reached through the execution, never through the client

> Revised by [ADR-0019](0019-a-client-owns-its-session.md): the session moves onto `WsClient`, whose
> lifetime it matches, once `WsClient::extensions` is removed. The reasoning below holds only while
> that field is present.

`WsContext::session()`, plus a `Session` extractor for handlers. It is not a field on `WsClient`.

`WsClient` already carries the execution's bag. A second field beside it, distinguished from the first
by name alone, is the arrangement that produced three bags across a single connect. Two lifetimes get
two access paths.

### The session bag is its own type

`Extensions` does not encode its lifetime, so two of them are the same type and nothing stops a value
meant for the connection being written to the execution. A newtype does not prevent that either, but it
puts the lifetime in every signature and binding that carries one, which is where a reader looks.

### Created before the guards, dropped with the client

The session is created in `begin_connect` before the connect execution's guards run, so a guard can
write to it. It is dropped when `handle_disconnect` removes the client. `Drop` is the teardown; no hook
is added, and nothing requiring asynchronous cleanup belongs in the bag.

A reconnect produces a new session. This is a connection's lifetime, not a user's.

### Disconnect becomes an execution

`GatewayTrait::on_disconnect` gains a context, so teardown reads the session the way every other
participant does. It gains no enhancer chain: rejecting a disconnect is meaningless, and a guard there
would be a trap.

### No injectable session, and no session provider scope

Every participant that would read the session already holds the context — guards, interceptors, pipes
and error handlers receive `&C`, and a handler takes the extractor or the context itself. An injectable
would be a second path to something already reachable.

A service *below* those cannot reach the session and is passed what it needs. That cost is the same one
already accepted for per-execution state, and it is not avoidable without an ambient scope, which
ADR-0016 declines on the ground that nothing in it is checkable.

Provider instances are not scoped to the session. That would need a second cache, a scope variant, and
elevation rules whose lifetime again follows a dependency graph — the non-locality ADR-0017 exists to
avoid. Nothing forecloses it: a session cache would sit on the session the way `ExecutionCache` sits on
the execution.

### WebSocket only

RPC calls are standalone, gRPC contexts are per-call, and HTTP has no framework-level connection.
`session()` is on `WsContext` and not on `HandlerContext`, where it would answer `None` on three
transports out of four.

The name is `session` rather than `connection` because a gRPC bidirectional stream is the shape that
could want one later, and one HTTP/2 connection carries many streams.

## Consequences

- `WsContext` gains `session()` and becomes the only context carrying two bags. The asymmetry is the
  point: WebSocket is the only transport with a session.
- `WsContext::new` takes the session handle, since every execution on a connection must receive the
  same one.
- Breaking for `GatewayTrait::on_disconnect` implementors, which gain a context parameter.
- Two bags of the same underlying type live one call apart, separated by their access paths. The
  newtype narrows the mistake rather than removing it.
- A handler or enhancer that reads the session and needs a service to act on it passes the value down.
- Nothing gains connection-scoped provider instances.

## Roads not taken

**Keying the scope on the client, as Nest does.** It yields connection-lifetime state for free and
costs per-message scope entirely, because one mechanism cannot express both lifetimes at once. The
per-message boundary is worth more than the free connection state.

**A `session` field on `WsClient`.** The shortest path to a handler, and the one that puts two
lifetimes on one struct behind two similarly-named fields.

**An injectable `Session`.** Every place it would be injected already holds the context.

**`session()` on `HandlerContext`.** Universal reach at the price of a method three transports answer
`None` to, which is the hollow shape ADR-0016 removed from `ProviderContext`.

**A teardown hook.** `Drop` covers a bag of values. A hook would invite the asynchronous cleanup this
store is not built to run.
