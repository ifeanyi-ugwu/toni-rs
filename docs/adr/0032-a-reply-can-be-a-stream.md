# 0032 — An RPC reply can be a stream, and the execution rides it

Status: proposed

## Context

An RPC handler answers with at most one value. `RpcHandlerResult` is
`Result<Option<RpcData>, RpcError>`, `RpcMessageCallbacks::message` hands every adapter exactly that
shape, and all seven client transports resolve a call through a `oneshot` slot that the first reply
frame consumes. Nothing in the stack can carry a second frame for one call.

Every other transport streams. HTTP has streaming bodies and SSE, WebSocket has
`WsHandlerOutput::Stream`, gRPC has tonic's native modes. Nest's `@MessagePattern` handler returning
an Observable emits multiple responses to the reply subject, its client's `send()` is
multi-emission, and unsubscribing sends an upstream disposal notice.

ADR-0016 made the execution end when the answer ends, and ADR-0021 fires the cancellation token when
a scoped body or stream is dropped before its last frame. Both name RPC's obligation the moment a
stream variant exists: the context rides the drain, and an abandoned stream fires the token.

Two facts about the current wire constrain the design:

- Reply framing is seven private copies of one convention: `error_status` appears in every adapter,
  and `frame_response`/`frame_panic`/`parse_response` are byte-identical across the four broker
  `wire.rs` modules. A grammar change is seven lockstep edits.
- `{"response": null}` means "an event handler acknowledged a request-shaped call" on every
  transport. The end of a stream cannot alias it.

## Decision

### The output is an enum, and its stream items are fallible

```rust
pub enum RpcHandlerOutput {
    Empty,                                                  // fire-and-forget: nothing goes back
    Single(RpcData),                                        // one reply
    Stream(BoxStream<'static, Result<RpcData, RpcError>>),  // many frames, then an end marker
}
pub type RpcHandlerResult = Result<RpcHandlerOutput, RpcError>;
```

The item type mirrors what the wire can express. A WebSocket frame has no error channel — an error
to a WS client is another message the gateway shapes — which is why `WsHandlerOutput::Stream`
carries bare `WsMessage`. An RPC call has a correlation and a canonical error envelope, so a stream
can fail mid-flight and say so. An `Err` item ends the stream, and the two-lane rule extends per
item: `AppError` renders as a final item carrying the canonical envelope followed by a clean end —
the client sees data — while `PatternNotFound`/`Forbidden`/`Internal` render as an error end that
the client's stream yields as its failure.

A streaming handler is an ordinary `#[message_pattern]` handler that declares `-> RpcHandlerResult`
and constructs the `Stream` variant, the same move a gateway makes with `WsHandlerOutput::Stream`.
No new pattern attribute, no return-type detection.

### The execution rides the stream, and a drop before the end fires the token

`ScopedRpcStream` is the WebSocket shape: it holds the `RpcContext` by value — the context is the
keep-alive, its shared state carrying the execution cache and the token — polls through to the inner
stream, and marks itself drained when the stream ends. Dropped before that, it fires `cancel` on the
execution's token. The wrap happens after the interceptor chain returns, so a stream substituted by
an interceptor is scoped too.

### Core speaks the grammar and drives the drain; adapters carry the bytes

A `toni::rpc::wire` module owns the framing in both directions — single replies, stream items, end
markers, error rendering, panic frames, parsing — and a drive loop that drains a handler's stream
through a transport-supplied frame sender. The seven copies collapse into it. An adapter contributes
its carrier: a closure that puts bytes on its reply channel (TCP and UDP splice `"id"` there), a
subscription for cancel notices, and its native correlation. A send failure stops the drive and
drops the stream, and the token fires.

### The wire grammar

Single-reply frames do not change. Stream frames are new keys:

| Frame | Body |
| --- | --- |
| item (`Json`/`Text`) | `{"stream": <value>}` |
| item (`Binary`) | `{"stream_b64": "<base64>"}` |
| clean end | `{"end": true}` |
| error end | `{"end": true, "err": {"message", "status"}}` |

TCP and UDP splice `"id"` into every frame. An item frame must be JSON to be distinguishable from
the end marker, which rules raw binary out; base64 under its own key cannot collide with user JSON.
`{"response": null}` keeps meaning acknowledged-with-nothing and never marks a stream's end.

A stream-aware client calling a single-reply handler treats the one `{"response": <v>}` frame as an
item followed by an end. A single-reply `send()` that receives a `stream` or `end` frame fails with
an error naming the mismatch.

### The client drops its stream, and the server hears it

Publishes to a broker rarely fail, so a server streaming to a departed client would never learn from
the carrier alone. The client sends a cancel notice when its reply stream is dropped before the end
— Nest's disposal notice. The server registers every request-shaped call in an in-flight registry
(correlation → abort handle) at message receipt and removes it when the task finishes. A cancel
notice aborts the task: mid-handler, the handler's future is dropped; mid-drain, the
`ScopedRpcStream` is dropped. Either way the token fires. TCP and UDP also abort everything in
flight for a connection when its read loop ends.

| Transport | Cancel carrier | Correlation key |
| --- | --- | --- |
| TCP | `{"id": …, "cancel": true}` on the same connection | `id` |
| UDP | the same frame, a datagram to the server socket | `(source, id)` |
| NATS | subject `toni.rpc.cancel`, no queue group — the owner acts | reply inbox |
| Redis | channel `toni:rpc:cancel` | reply channel |
| RabbitMQ | fanout exchange `toni.rpc.cancel`, an exclusive auto-delete queue per instance | `correlation_id` |
| MQTT | topic `toni/rpc/cancel`, QoS 1 | correlation data |
| Kafka | topic `toni.rpc.cancel`, a unique consumer group per instance | `correlation_id` |

Broker cancel bodies are `{"cancel": true, "key": "<correlation>"}`.

### The client opens a stream with its own verb

`RpcClientTransport` gains `open_stream`, defaulted to return a `StreamingUnsupported` error so
existing implementations keep compiling. `RpcClient::stream` and `RpcRequest::stream` sit beside
`send`, with `stream_json` parsing each item. The reply is a core-provided `RpcReplyStream` whose
`Drop` before the end sends the cancel notice — written once, not per transport. For a stream, the
transport's `with_timeout` bounds the gap to the next frame, the first included.

### The call info travels whole

`RpcControllerWrapper::handle_message` takes `RpcData` and the `RpcCallInfo` rather than fields
picked off it. The wrapper builds the context from the info, and the info's extensions seed the
execution's bag — the field exists today with no writer and no reader.

## Consequences

- `Ok(Some(data))` becomes `Ok(RpcHandlerOutput::Single(data))` and `Ok(None)` becomes
  `Ok(RpcHandlerOutput::Empty)` — in handlers written against the raw shape, in interceptors, and in
  tests matching on the result. `From<RpcData>` and `From<Option<RpcData>>` keep `.into()` working
  where the old shapes flowed.
- Error handlers keep answering `RpcData`, and a claim renders as `Single`. Error handlers do not
  stream.
- An interceptor sees `Stream` as an opaque variant. One that replaces it discards the stream before
  the scoped wrap, so no token fires — the WebSocket behavior.
- A mid-stream `Err` item reaches neither `ErrorObserver`s nor the error-handler chain: the pipeline
  returned before the tail existed, and `Drop` is synchronous — the ADR-0021 gap at a second seam.
- Cancel is best-effort on brokers: a notice racing the registration is dropped. MQTT QoS 1 can
  duplicate items on reconnect. A lost UDP datagram can take the end or the cancel with it; the
  client's per-frame timeout is the backstop.
- An adapter that does not yet speak the stream grammar answers `Stream` by dropping it — the token
  fires — and framing a wire error naming the unsupported transport.

## Roads not taken

**A multi-emission `send()`, as Nest has.** Rust separates the one-value and many-value cases in the
type system; collapsing them into one verb would make every single-reply call carry a stream's API.

**`#[stream_pattern]`, or detecting `impl Stream` returns.** The enum in the signature says
everything the attribute would. The SSE macro's stream detection stays available as sugar if writing
the variant proves noisy.

**Raw binary item frames.** An item frame must be JSON, or the end marker would need a
length-prefixed binary framing of its own on every transport.

**Reusing `"response"` for items.** A pre-stream client would resolve its `oneshot` on the first
item and read a partial answer as the whole one. A new key makes the mismatch loud.

**Per-item error-handler claims, or observers fanned from the tail.** Both need an async seam inside
the adapters' drive loops, and ADR-0021 records why the drop side cannot fan an async observer.
