# 0021 — Cancellation signals the tail, because the handler is already covered

Status: accepted

## Context

`CancellationToken` is complete. It carries `cancel`, `is_cancelled`, and an async `cancelled` that
composes with `select!`, and `HandlerContext::cancellation` puts it on all four contexts.

Nothing has ever fired it. `cancel` has no call site outside the token's own tests, and no code
anywhere reads the flag or awaits the future. The `Cancelled` framework event names its producer in
its own doc comment — *"disconnect before the handler finished, or a deadline / cancellation token
firing"* — and has none either.

### A dropped future is the answer to the common case

Nest needs an `AbortSignal` because a Node promise keeps running after the socket closes. A Rust
future stops when it is dropped, and the server drops the response future when the connection dies:

```
a_disconnect_drops_the_handler_future     client goes away  → handler's sentinel drops
a_live_connection_does_not_drop_the_handler  client waiting → sentinel intact
```

The second is what makes the first mean anything, and it is easy to get wrong: a control built on
`timeout` drops the request future, which closes the connection, and so controls for nothing.

For a handler still running when the client leaves, there is therefore nothing to signal. The work
has already stopped. That is why four transports have carried an unfired token without anyone
noticing.

### What a dropped future does not reach

Work that escaped the handler future:

- **The streaming tail.** The handler has returned and the body is draining. Dropping the body stops
  the polling, and a task feeding that body — a channel producer, a cursor — learns nothing.
- **Deliberately detached work**, where a handler spawned something holding a context clone.

ADR-0016 made both part of the execution by keeping the context alive across the drain. That is what
makes a producer possible, and it is the only thing a token buys here.

## Decision

### A scoped body or stream dropped before it finished fires the token

`ScopedBody` and `ScopedStream` already hold the execution through the tail. Each tracks whether its
inner body or stream reached the end, and a `Drop` before that fires `cancel` on the execution's
token.

A body dropped before its last frame is the disconnect signal — the analogue of Node's
`writableFinished`, arriving where Rust does not already answer.

### Nothing else produces it

A buffered response needs no producer: the handler future is dropped, and nothing that could observe
the token is still alive to. A guard or interceptor that wants to stop work of its own returns
instead, which ADR-0016 settled.

### The `Cancelled` event stays without a producer

`Drop` is synchronous. `ErrorObserver::observe` is `async`, and toni core holds no runtime handle to
spawn one from. So the token can be fired where the signal is known and the event cannot be fanned
from the same place.

Recorded as a gap rather than closed quietly: an observer-visible cancellation needs either an async
seam in the adapter's drain or a synchronous observer variant, and both are larger than this.

### `deadline()` is a different feature

`GrpcContext` could populate it from `grpc-timeout`, which is header parsing and unrelated to a
dropped body. It stays unimplemented here.

## Consequences

- A streaming handler can `select!` on `ctx.cancellation().cancelled()` and stop feeding a body
  nobody is reading.
- `ScopedBody` and `ScopedStream` gain a completion flag and a `Drop` impl, so a wrapper that was
  inert becomes load-bearing.
- The token stays silent for buffered responses, which is honest: nothing there could hear it.
- `Cancelled` remains producerless, and the reason is now written down rather than implied.

## Roads not taken

**An `AbortSignal` on every request, as Nest has.** Firing on any disconnect regardless of whether
work survives it would spend a signal on the case the language already handles, and teach handlers to
check something that is almost never true.

**Spawning from `Drop` to fan the event.** Puts a runtime handle in core to deliver an observer call
whose ordering nothing could rely on.

**Cancelling from the dispatcher when a guard rejects.** A rejection is an answer, not a
cancellation, and ADR-0016 already made answering the way a participant stops the chain.
