# 0023 — Extraction is one trait, and body-freedom is a convention

Status: accepted

## Context

ADR-0015 made `FromContext<C>` the trait a handler parameter is read through, and kept the two
HTTP-only traits that preceded it as shorthands beneath it: `FromRequestParts`, sync and handed
`&RequestPart`, and `FromRequest`, async and handed the owned `HttpRequest`. The decision was
recorded as deferred — two spellings would be judged once WebSocket and RPC extractors existed.

They exist now, and neither transport grew a shorthand. A `FromContext<WsContext>` impl is five
lines and nobody asked for a shorter one. So HTTP had three ways to write an extractor and the other
transports had one.

### The narrower trait proved nothing the framework reads

`FromRequestParts` receives `&RequestPart` and so cannot reach the body. That reads as a
compile-time guarantee, but nothing consumes it. The controller macro classifies a parameter by the
last segment of its written type, so a custom extractor is `Unknown` whichever trait it carries, and
the one-body check exempts `Unknown` outright — several custom metadata extractors have to be able
to share a handler. The guarantee reaches the extractor's author and stops there.

Reading metadata without touching the body never needed a trait either. `HttpContext::request`
borrows the parts, and an extractor written directly against `FromContext` that calls it is exactly
as body-free as one written against `FromRequestParts`.

### The shorthand cost an extra impl on the one case that is hard

A custom body extractor wrote two impls: `FromRequest` for the reading, and a `FromContext` impl
whose entire body was `extract_body::<Self>(ctx).await`. ADR-0015 recorded those six lines as the
migration cost for existing body extractors. They were a permanent cost, paid by every new one.

### Two blanket-adjacent impls that could only just coexist

The `FromRequestParts` blanket over `T` and the concrete body-extractor impls coexisted only
because no body extractor implemented `FromRequestParts`. An `impl FromRequestParts for Json<T>`
added for any reason would have collided, and `rustc` would have reported the overlap against two
impls in `from_context.rs` — a file whoever wrote the new impl had not opened.

## Decision

### `FromContext` is the only extraction trait

`FromRequestParts` and `FromRequest` are removed. Every HTTP extractor implements
`FromContext<HttpContext>`, the same shape a WebSocket or RPC extractor has. No blanket impls
remain in the extraction path, so the overlap above is not expressible.

### Metadata is borrowed from the context

An extractor that reads headers, path params, query params or extensions calls `ctx.request()` for
`&RequestPart` and does not touch the body. `Path` and `Query` are written this way, and any number
of them run on one handler.

### The body is taken, and the taking names the second asker

`take_body::<Self>(ctx)` yields the request once. The second caller gets a `BodyAlreadyRead`
carrying the extractor's name, which `?` lifts into `BodyExtractionError` and which is logged at
error level — the request was well-formed and the fault is the application's. A body extractor is
now one impl:

```rust
impl FromContext<HttpContext> for MyRawBody {
    type Error = BodyExtractionError<MyError>;

    async fn extract(ctx: &HttpContext) -> Result<Self, Self::Error> {
        let req = take_body::<Self>(ctx)?;
        // read `req`, mapping failures through `BodyExtractionError::Extract`
    }
}
```

### Body-freedom becomes a convention

What is given up is precise: an extractor author can now call `ctx.take_request()` while meaning to
read a header, and the handler's body extractor then fails on a live request rather than the
mistake being unwritable. That failure is diagnosed by name and logged, and the alternative bought
a guarantee no part of the framework reads.

## Consequences

- Breaking for every custom extractor. A metadata one changes its signature and reads
  `ctx.request()`; a body one folds its two impls into one. `toni::FromRequestParts` and
  `toni::FromRequest` are gone from the crate root, replaced by `toni::FromContext` and
  `toni::take_body`.
- `BodyExtractionError::AlreadyRead` carries a `BodyAlreadyRead` rather than a bare name, so the
  same value serves `take_body`'s own result and the wrapped one.
- `Request` gains an inherent infallible `from_parts`. Its provider and factory build one while
  holding parts and no context, which the trait had forced through a `Result` that was
  `Infallible` and an `.expect` that could not fire.
- A metadata extractor's `extract` is `async` with nothing to await. Its unit tests await it.
- ADR-0015's "Two shorthands, and why one of them is not a blanket" is superseded. The rest of
  0015 stands: one trait generic over the context, extraction reading from that context, and the
  error type belonging to the extractor.

## Roads not taken

**Keeping `FromRequestParts` for a macro that classifies by trait.** The one use left for the trait
would be a controller macro that asks whether a custom extractor implements it, replacing the
by-type-name table and extending the one-body check to custom types. A proc macro cannot ask: it
runs before name resolution, and receives tokens rather than types. Autoref specialization answers
the question inside generated code, but the check is an arity rule over the whole signature, and
the diagnostic it exists to produce names the two colliding parameters and their spans — neither
survives being raised from inside a macro's own output.

**Keeping `FromRequest` alone, without `FromRequestParts`.** It would remain a nameable bound for
`extract_body` and one public trait fewer. It also remains a second impl per body extractor for no
capability, since `FromContext` reaches the owned request through `take_body` just as well.
