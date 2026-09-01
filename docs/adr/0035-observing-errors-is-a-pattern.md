# 0035 — Observing errors is a pattern, not a pipeline role

Status: proposed

## Context

`ErrorObserver` was a registerable async trait, fanned out at six sites across the four dispatchers:

```rust
Self::fan_out_observers(observers, observed_err, context).await;
for handler in error_handlers.iter().rev() { … }
Self::safe_render(|| http_err.to_response(), observers, context).await
```

The fan-out runs before the response is built, one observer at a time, awaited. Its documented
contract — fire-and-forget, no return value, no way to shape the response — reads as costing the
request nothing, and the code charges the client every microsecond an observer spends. A logging
observer that reaches for the network puts that network call in front of the client's response.

Making it honest means taking the fan-out off the request path, and there are two ways:

- **Detach it.** `tokio::spawn` needs a runtime handle in core, which core does not have and does not
  want (the WebSocket adapters exist because of that rule).
- **Run it after the response is on the wire.** An HTTP response is returned up the stack, not sent
  from inside the dispatcher, so there is no post-send point to hang it on without threading a
  callback through the adapter boundary — for four transports, to deliver an event that in a default
  app has nobody listening.

What the observer alone could see is two things: a panic inside a `#[catch]` handler and a panic
inside a response renderer. Both are failures of the chain machinery rather than extension points —
an application cannot fix a panicking renderer, only be told about it — and both were already silent
in every app that never called `use_global_error_observer`, which is every app by default.

Everything else the observer saw, the error-handler chain sees as well, typed and downcastable, with
`None` available to decline: handler errors (`AppError` unwrapped to the domain error), guard
rejections, middleware failures, unrouted calls, and panics in guards, interceptors and handlers.
The observer is a second delivery mechanism for values the chain already delivers.

## Decision

**Remove the role.** `ErrorObserver`, `use_global_error_observer`, `fan_out_observers`, and the
`error_observers` field threaded through the HTTP, RPC, WebSocket and gRPC dispatchers all go. The
two machinery failures each get one `tracing::error!`: on by default rather than opt-in, capturable
by any subscriber, and off the request path with a non-blocking one.

**Observing errors becomes a pattern.** An `ErrorHandler` that returns `None` sees every error the
chain sees and shapes nothing; from there it publishes onto a transport the app already speaks, and a
consumer does the observing. `examples/error_telemetry.rs` is the worked one, over RabbitMQ.

That is a better answer than detaching would have been. A detached task dies with the process and
scales with it; a published message survives both, and the consumer can be slow, restarted, or
scaled on its own. toni ships seven RPC transports, so the pattern costs an app one handler and one
`#[event_pattern]`.

**`Cancelled` goes with it.** The event existed only to be delivered through the observer, and
ADR-0021 recorded that it never got a producer: `Drop` is synchronous and `observe` is async, so the
one place that knows a client gave up could not raise it. The cancellation token is the signal, and
it is strictly more capable — a holder can `select!` on it and stop work, rather than be told after
the fact.

## Consequences

- Breaking for any app calling `use_global_error_observer`, and for anything naming
  `toni::errors::Cancelled`.
- A panicking `#[catch]` handler and a panicking renderer are logged, and the pipeline carries on as
  before: the next handler for the first, a hardcoded envelope for the second.
- An RPC guard panic is refused at the dispatcher rather than routed through the chain, so the wire
  `forbidden` frame and the log are the whole of its signal. HTTP and WebSocket route a guard panic
  through the chain, where `#[catch(PanicRecovered)]` still claims it.
- `PipelineSegment` stays: it is carried on `PanicRecovered`, which the chain still delivers.
- Error telemetry has three shapes, all of them already supported: log inside a declining `#[catch]`,
  install a `tracing` layer, or publish and observe in a consumer.

## Roads not taken

**Detaching the fan-out with `tokio::spawn`.** Fixes the latency and breaks the no-runtime-in-core
rule. Feature-gating it does not help: a gated spawn still needs a reactor, so the gate buys nothing
but a second configuration a user can get wrong.

**A post-wire hook per transport.** Coherent, and a dispatcher restructure across four transports to
keep a trait whose unique coverage is two failures nobody registers for.

**A default-registered logging observer.** Identical behaviour to the `tracing::error!` that replaced
it, reached through a trait, a `Vec`, a registration call and a per-item `catch_unwind`.

**A synchronous observer variant, so `Drop` could produce `Cancelled`.** A whole new trait, a
registration path and a global-registry entry, to deliver one notification nobody asked for — and the
token already tells whoever cares, at a point where they can act on it.
