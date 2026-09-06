# 0036 — A refused WebSocket connection is told why

Status: accepted

## Context

Connect guards run after the handshake. By the time a guard says no, the HTTP exchange is over and
the socket is a WebSocket, so there is no status left to refuse with. Every adapter answered that by
returning:

```rust
let client_id = match callbacks.connect(parts, sender.clone()).await {
    Ok(id) => id,
    Err(_) => return,
};
```

The `WsError` the guard produced was not even bound. Returning drops the socket, so the caller gets
an abrupt close carrying no code and no reason, indistinguishable from a crashed server or a dead
network. The framework logged the refusal at `error` because the log was the only place the fact
survived.

Refusing earlier, during the handshake, is the shape an HTTP developer reaches for and it is worse
here: browsers deliberately withhold the status and body of a failed WebSocket upgrade, so a `403`
reaches a JS caller as a bare error event with nothing in it.

## Decision

**Answer a refusal on the socket the handshake just opened, then close it.** Two frames: the
canonical envelope as text, then a close carrying an RFC 6455 code.

- 1008 Policy Violation for a refusal the caller caused — a guard saying no, an unauthorized
  connect. RFC 6455 has no auth-specific code, and 1008 is the one it reserves for "you broke a
  rule".
- 1013 Try Again Later when the kind is `TooManyRequests`.
- 1011 Internal Error for a refusal the server caused, which is what a panicking connect guard now
  produces: the event stays typed as `PanicRecovered` through the refusal, so its kind decides the
  code.

A browser reads `event.code` and `event.reason` off `onclose` without parsing anything, and an
application client reads the envelope frame it already parses everywhere else.

**A subprotocol that defines its own refusal codes closes with those, and sends nothing else.**
`WsError::Refused { code, reason }` says exactly that. The graphql-ws gateway uses it to close with
4406, the code `graphql-transport-ws` reserves for a subprotocol it cannot speak — a client that has
not agreed on the grammar has nothing to parse an envelope with.

**The log follows the panic policy of ADR-0035.** Now that the caller is told, a refused connect is
narrated at `debug` like every other guard outcome, and `error!` goes back to meaning "nobody could
be told".

## Consequences

- `WsMessage::Close` carries `Option<CloseFrame>` rather than nothing, so every adapter maps a code
  and reason onto its native close type. `WsMessage::close()` still builds a bare close;
  `close_with(code, reason)` builds one that explains itself, truncating the reason to the 123 bytes
  RFC 6455 leaves for it.
- A client that connected and saw silence now sees one text frame and a close. Anything asserting
  "nothing arrives on refusal" sees the envelope instead.
- `close_code` and `refusal_frames` live in core beside `render_error`, so the five WebSocket
  adapters carry the policy rather than each inventing one.
- The connect guard stops being the exception in the panic table: every transport now delivers a
  refusal to whoever asked for it.

## Roads not taken

**Refusing before the upgrade with an HTTP status.** The guards need a `WsContext`, which is built
around a sink that does not exist until the socket does. It would also tell a browser client less
than the close frame does.

**Close-only, with no envelope.** Half the information: the code says *category*, the envelope says
*which rule and why*. The exception is a subprotocol refusal, where the envelope is unreadable by
definition — which is what `WsError::Refused` marks.

**Reusing the per-message refusal path.** A refused message answers on a live connection and the
socket carries on; a refused connect has no session to carry on with. Sharing the rendering
(`to_message`) is the part worth sharing, and it is shared.
