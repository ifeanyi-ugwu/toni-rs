# Divergences: race 2a's pre-compile batch, `ulo-http`

The user-visible choices made building T1 with `NoRoute`, T8, T14 and the `Middleware` signature
(`transports/RESPONSE.md`, "Fourth response"), each with what the decision or `transports/DESIGN.md`
says, what was written, and why. All await sign-off.

Files: `crates/ulo-http/src/{miss,render,service,middleware,pre_dispatch,cors,backend,server,limits,lib}.rs`.
`miss.rs` is new. `tower_bridge.rs` is unchanged.

## 1. A 405 reaches the error handlers as a `CallError` of kind `BadRequest` (T1)

- **Decision:** a public `ulo_http::MethodNotAllowed` with `allow()`, downcast from the `BoxError` or
  from a `CallError`'s source; the choice left to this batch. DESIGN §3.2 now reads "the error handlers
  receive it as `fw_http::MethodNotAllowed`", which reads as the bare type.
- **Written:** the router's 405 is offered as `CallError::new(BadRequest, <text>)` whose source is
  `MethodNotAllowed`, the text being the one the 405 already carried ("this path does not answer
  `POST`; it answers GET, HEAD, OPTIONS"). An error handler reads it as
  `err.downcast_ref::<CallError>()?.source_as::<MethodNotAllowed>()`, the same pattern as `NoRoute`.
- **Why wrapped:** an app reshaping every error calls `CallError::from_boxed`, which answers `Internal`
  with the message withheld for any type it does not recognise. Bare, an unrecognised 405 would reach
  such an app's envelope as a 500. Wrapped, it reaches it as a client error with the text above, and
  only the exact status and `Allow` need the source.
- **Why `BadRequest`:** no kind is 405. `BadRequest` is the generic client-error kind, and an
  extractor's 413 and 415 already ride on it, with the exact status applied at rendering while the
  kind is unchanged. `NotFound` claims the resource does not exist; `Unimplemented` maps to 501, a
  server error that monitoring counts as a fault.
- **Cost:** an error handler that claims every `BadRequest` now also claims 405s. It sees the
  `MethodNotAllowed` source if it looks.
- **Design text:** §3.2's sentence and the §12 table row ("405 with `Allow` as
  `fw_http::MethodNotAllowed`") would read "a `CallError` of kind `BadRequest` whose source is
  `fw_http::MethodNotAllowed`".

## 2. `MethodNotAllowed`'s surface (T1)

- **Written:** `method() -> &Method`, the request's method, and `allow() -> &HeaderValue`, the `Allow`
  value as the router computed it, so a handler answering its own 405 inserts it as is. `Clone`,
  `Debug`, `Display`, `Error`. Its fields are private and its constructor crate-private: only the
  router makes one.
- **Re-export:** `ulo_http::HeaderValue` joins `HeaderMap`, `Method` and `StatusCode`, since
  `allow()` returns one.

## 3. `NoRoute` (T1)

- **Decision:** the 404 stays `CallError::new(NotFound, ..)`, with a public `NoRoute` marker as its
  source.
- **Written:** as decided; the message is unchanged ("no route matches this path"). `NoRoute` is
  `Clone + Copy + Debug + Error` and displays the same text. A private field keeps it unconstructible
  outside the crate, so its presence means the router decided the miss.

## 4. How an unclaimed miss renders (T1, T14)

- **Written:** the 405 document moved to `render.rs` as a private `method_not_allowed`, built on the
  shared problem-document writer. `service.rs` no longer builds one and hands an unclaimed miss to
  `render::render_error` like any other error. `render_error` recognises `MethodNotAllowed` bare or as
  a `CallError`'s source and renders 405 with `Allow` while the error is of kind `BadRequest`; an error
  handler that reshaped it to another kind is rendered by that kind, as 413 and 415 already were.
- **Body:** `{"type", "title", "status", "detail"}` in that order, as every other problem document.
  The previous 405 was written through `serde_json::json!`, whose keys came out alphabetically.
- **413 and 415:** their recognition now reads the `ExtractError` off the converted `CallError`'s
  source rather than off the raw error. The cases agree: a bare `ExtractError` converts to a
  `CallError` whose source it is, a `CallError` keeps its source, and an `ExtractError` behind
  `Redacted` or `LookupError` is recognised by neither.

## 5. A route timeout runs the error handlers under a grace (T8)

- **Decision:** when the timer wins, drop the pipeline, cancel with `Deadline`, run the error handlers
  with a `Timeout` `CallError` through `ulo::recover` with the matched handler under a grace of about
  one second in the core's `Bound` vocabulary, and render canonically when the grace runs out.
- **Written:**
  - `HttpConfig::timeout_grace: Bound`, `Bound::Default` unset, which is one second.
    `Bound::After(d)` is `d`; `Bound::Unbounded` waits for the handlers. Set with
    `Server::timeout_grace(Bound)`, since `HttpConfig` is `#[non_exhaustive]` and the builder is how
    every other field is set. Timed by the app's `Timer`, which `listen()` guarantees.
  - The handlers run with the matched handler's tiers, then the global ones, and see
    `cx.exec().cancel_reason() == Some(Deadline)` and `cx.exec().handler()` set.
  - Their answer is the response, its headers merged with the ones they wrote. An error they return
    is rendered as it stands: an unclaimed `Timeout` is 504 with the same text as before, and a
    reshaping to another kind renders by that kind. The `Deadline` rule that turns any error on a
    cancelled execution into `Timeout` does not apply here, since the error offered already says
    `Timeout` and a reshaping of it is the handler's to make.
  - When the grace passes first, the handlers' future is dropped at its current await and the
    canonical 504 is sent, without the headers they wrote. With `Bound::After(Duration::ZERO)` only a
    handler answering on its first poll is heard.
- **The context they read:** built from the head as the scoped sub-step received it, after the
  unscoped entries and before the scoped ones, with the matched route, no body and no upgrade. A
  rewrite a scoped entry made is lost with the dropped pipeline. The head and connection are cloned
  before the pipeline takes the request, on routes with a timeout only.
- **`Timeout`'s doc** (`limits.rs`) and `AppService`'s step list state the new path.

## 6. `Middleware::handle` answers `Result<Response, BoxError>`

- **Decision:** the signature, `Next::run` answering the same `Result`, and a middleware's `Err`
  reaching the error handlers as a pre-dispatch failure, unscoped through `recover(None)` and scoped
  through `recover(Some(handler))`, the path a panic takes.
- **Written:** as decided. The `Err` is caught at the entry's own boundary, where its panic was
  already caught, and offered from there; a by-type middleware's lookup failure and a layer's `Err`
  take the same path, as before.
- **`next.run(req)` is always `Ok`.** A failure further in has reached the error handlers and been
  answered or rendered at its own entry, and that response is what the outer middleware receives.
  An outer CORS entry therefore adds its headers to the 401 an inner authentication middleware's
  `Err` became, which a browser needs to read the 401. Propagating the inner `Err` outward instead
  would skip every outer middleware's response handling on a failure, and move a scoped entry's
  failure to the unscoped side, where only the global handlers apply. The `Result` keeps
  `next.run(req).await` a middleware's answer as it stands, and `?` on it compiles.
- **`Cors`:** a preflight answers `Ok`; otherwise `next.run(req).await?` and the headers, as before.
- **Tower:** unchanged. A layer's `Err` already reached the error handlers through the entry's
  boundary; `PreDispatch::layer`'s doc now says it does so as a middleware's `Err` does.
  `ulo_http::Service`, the inner service a layer wraps, stays `Infallible`, as the rest of the chain
  always answers.

## For other areas

- **Design (DESIGN §3.2 and the §12 table):** entry 1's wording for the 405's representation.
- **Design (DESIGN §3.3):** "they can answer without calling `next`, which is how CORS preflight and
  authentication work" predates the `Err` path; authentication now returns `Err`.
