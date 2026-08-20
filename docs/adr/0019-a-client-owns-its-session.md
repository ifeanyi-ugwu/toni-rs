# 0019 — A WebSocket client owns its session; an execution owns its bag

Status: proposed

Revises one decision in [ADR-0018](0018-a-connection-is-a-session.md), which put the session behind the
execution and kept it off `WsClient`. The rest of 0018 stands: the session is a store rather than a
context, it has a connection's lifetime, `Drop` is its teardown, a disconnect is an execution, and
there is no injectable and no session provider scope.

## Context

### The object with a connection's lifetime holds an execution's state

`WsClient` exists once per connection. Its `extensions` field is the *execution's* bag, repointed on
every context construction:

```rust
client.extensions = shared.extensions.clone();
```

A per-connection object carrying per-execution state is the mismatch, and it is not incidental. That
line is what produced three bags across one connect: it repoints a *clone*, the connection stored the
original, and a third context was built for the hook. Each bag was reachable and none of them agreed.

### The field it was avoiding a clash with has no readers

ADR-0018 declined to put the session on `WsClient` because it would sit beside `extensions` and the two
would be told apart by name alone. That reasoning holds only while `extensions` is there.

`WsClient::extensions` has one reader in the repository, a test, and one writer, the line above. A
handler that wants the execution's bag takes `Extensions` as a parameter, which is how every other
transport reads it and what ADR-0015 established. The field is a delivery mechanism that the extractor
superseded.

### Two owners for one bag

The execution's bag is reachable through the context and through the client. Nothing keeps them in
agreement except the assignment that repoints one to the other, and that assignment is exactly where
the disagreement came from.

## Decision

### The client owns the session, and is born with it

`WsClient::new` creates the session, so a connection and the state scoped to it come into being
together and there is no window in which one exists without the other. `WsContext::session` reads it
from the client.

### The execution's bag has one owner

`WsClient::extensions` is removed. The bag lives on the context, read as `ctx.extensions()` or taken as
an `Extensions` parameter, on WebSocket exactly as on the other three transports.

`WsContext::new` no longer assigns to its client. No clone of a client can disagree with another about
which bag is current, because there is nothing to point.

### Reached through an accessor, not a public field

`session()` returns a reference; the field is private. The values inside are still writable — the store
is a handle with interior mutability — but the handle itself cannot be replaced.

A public field would leave `client.session = Session::new()` expressible on a clone, which is the
shape of the defect this ADR is unwinding. Making it unrepresentable is worth more than the symmetry
with how `extensions` used to be exposed.

## Consequences

- `WsClient` gains `session()` and loses `extensions`. Breaking for anything reading the latter, which
  in this repository is one test.
- The wrapper's client map holds `WsClient` again rather than a client-and-session pair: the client
  carries its own session, so there is nothing to keep beside it.
- `WsContext::new` takes no session argument and mutates nothing it is given.
- `ctx.extensions()` and `ctx.session()` read the same way at a call site and differ only in lifetime,
  which is the distinction a reader needs to make.
- A connect guard's write still reaches `on_connect`, through the context rather than through the
  client. The property is unchanged and the mechanism is not.

## Roads not taken

**Keeping `extensions` on `WsClient` alongside a session.** Two bags on the object whose lifetime
matches only one of them, distinguished by field name. This is what ADR-0018 correctly refused, and
removing the first field is what makes room for the second rather than repeating the arrangement.

**A public `session` field.** Symmetric with how `extensions` was exposed, and it preserves the one
assignment that has already caused a defect here.
