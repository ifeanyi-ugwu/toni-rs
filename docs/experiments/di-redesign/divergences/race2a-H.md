# Divergences: race 2a, H (HTTP requests, routing and replies)

Every place H's files in `crates/ulo-http` (`Cargo.toml`, `lib`, `transport`, `cx`, `body`,
`request`, `limits`, `router/**`, `extract/**`, `response`, `sse`, `render`, `__private`) depart
from `transports/DESIGN.md` or the spine, or fill what either leaves open. Each entry gives what
the design or spine says, what was written, and why. All await the user's sign-off. Requests to
other areas are at the end.

## Routing

### 1. Plan point: percent-decoding of path parameters

- **Plan:** H decides; `+` stays literal.
- **Written:** each request segment is percent-decoded as a path (RFC 3986), not as a form: `+`
  stays `+`, and `%2F` decodes to `/` inside its segment without splitting it. A `{*rest}` value
  is the raw remainder of the path, decoded the same way, so its `/` separators survive. A decoded
  byte sequence that is not UTF-8 is captured with U+FFFD in place of each invalid sequence. A
  static segment matches the request segment as sent or once decoded, so `/caf%C3%A9` matches the
  pattern `/café`.
- **Why:** `PathParams` holds `String`s (a frozen contract), and `Routed` has no error variant, so
  invalid UTF-8 can neither be kept nor refused. The other option, treating it as a miss, answers
  404 for a path that does name a route.

### 2. A parameter or a rest never captures an empty segment

- **Design:** silent.
- **Written:** `/u//x` does not match `/u/{id}/x`, and `/files//` does not match
  `/files/{*rest}`: each answers 404 unless another route matches.
- **Why:** the grammar already refuses an empty segment in a pattern (entry 3), and an empty
  `{id}` would reach `Path<u64>` as a 400 that names no fault the client can see.

### 3. The pattern grammar refuses an empty segment; both parsers report the same first failure

- **Spine:** the grammar is "a leading `/`; segments of static text, `{name}` with an
  identifier-like name, or a final `{*name}`; no brace elsewhere; no name twice".
- **Written:** also no empty segment (`/a//b`). A name is non-empty, starts with `_` or a letter
  and continues with `_`, letters and digits (Unicode, as Rust identifiers are). The repeated-name
  check runs segment by segment.
- **Agreed with HM:** `ulo-http-macros/src/pattern.rs` checks the same rules with the same reason
  texts in the same order, so a literal refused at compile time and a pattern refused at `prepare`
  (a prefix joined at runtime) read alike. A doc comment on each parser names the other.

### 4. Route precedence: the most specific pattern decides

- **Design:** silent on which route answers when two patterns match one path.
- **Written:** routes are ordered once in `prepare` by a key compared left to right: a static
  segment before a parameter, a parameter before a rest. `/users/me` answers before `/users/{id}`
  whatever the declaration order. The first pattern that matches decides the method check: its
  handler for the method, then `GET` for `HEAD`, then 204 for `OPTIONS`, then 405. A less specific
  pattern holding a handler for the method is not consulted, so `DELETE /users/me` with only
  `GET /users/me` and `DELETE /users/{id}` declared answers 405.
- **Why:** declaration order is invisible across modules. Path-first matching follows RFC 9110:
  the URI names the resource, and 405 means that resource does not support the method.

### 5. Conflicting parameter names are refused whatever the methods

- **Design:** "Two routes with the same method and the same normalized pattern, or two patterns
  that differ only in parameter names at one position … are a `Configure` error naming both
  handlers." The second clause names no method.
- **Spine:** `Pattern::conflicts`' doc spoke of "the same method".
- **Written:** as the design reads. `GET /u/{id}` with `DELETE /u/{name}` is refused, as is any
  pair that differs only in names, at one position or more. The error names both handlers as
  `Controller::method`.
- **Why:** one pattern groups its methods under one set of parameter names; with two, `HttpCx::param`
  would answer differently by method.

### 6. `Allow`

- **Design:** 405 carries `Allow` (RFC 9110); `OPTIONS` without a handler answers 204 with `Allow`.
- **Written:** the pattern's declared methods in declaration order, then `HEAD` placed after `GET`
  when only `GET` is declared, then `OPTIONS` when no handler declares it, comma-separated. It is
  computed once per pattern in `prepare`.
- **Why:** the router answers `HEAD` and `OPTIONS` itself, so both are allowed.

### 7. Plan point: a `HEAD` answered by `GET` drops its body

- **Plan:** H decides how.
- **Decided:** the router flags it as `Routed::Found { head_from_get: true }`, and the response
  then has its body replaced with an empty one, unread. A body of known length leaves its length
  as `Content-Length`, the header the `GET` would have sent (RFC 9110 §9.3.2), except on 1xx and
  204, where RFC 9110 §8.6 forbids one, and 304. A streaming body is dropped before its first item,
  so an `on_stream_end` callback sees it cut off.
- **Who does it:** step 8 of `AppService::call` is P's, and P wrote its own `without_body` in
  `service.rs`. It matches this decision except for the status exception; see the requests below.

### 8. `ScopePattern`'s semantics

- **Spine:** "a route pattern whose last segment may be `*`"; covers by "prefix match on segments,
  a parameter in the scope matching a parameter in the route".
- **Written:** a final `*` or `{*name}` is the wildcard, matching one or more segments, so
  `/admin/*` covers `/admin/users` and `/admin/{id}/notes` but not `/admin`. A scope's static
  segment covers only the same static segment. A scope's parameter covers a route's parameter or
  static segment, whatever the names, since it matches any one segment. A route's `{*rest}` is
  covered only by the scope's wildcard. `covers_path` applies the same rules to a concrete path.
  `ScopePattern` gains two private fields, built by `ScopePattern::parse`.

## Extraction

### 9. `Path`, `Query` and `Form` share one deserializer, and their failures name the field

- **Spine:** `Query` uses `serde_urlencoded` and fails as `Malformed { param: "query" }`.
- **Design:** `Missing` is for "a required header, query field or path segment"; each variant
  carries "the parameter's name so one pattern matches it".
- **Written:** a crate-private deserializer over name and value pairs (`extract/de.rs`) serves all
  three. Serde names a struct's fields with `&'static str`, which is what `param` needs: a missing
  field is `Missing { param: "<field>" }`, a value that does not parse is
  `Malformed { param: "<field>" }`, and a field given twice is `Malformed` naming it.
  `"path"`, `"query"` or `"body"` is the `param` only when serde names no field (a tuple, a
  scalar, a map).
- **Consequence:** `Option<Query<T>>` is `None` when a required field is absent, as `Option<P>`
  is for any `Missing`.
- **Query and form grammar:** the WHATWG URL standard's `application/x-www-form-urlencoded`
  parser: split at `&`, empty pieces skipped, split at the first `=`, `+` read as a space,
  percent-decoded, UTF-8 with U+FFFD for an invalid sequence. A struct, a map, an `Option`, a unit
  enum and a `Vec<(String, String)>` of the pairs deserialize. An empty value for an
  `Option<u32>` field is `Some` of a failed parse, `Malformed`, as with `serde_urlencoded`.
- **Manifest:** `serde_urlencoded` is removed from `ulo-http`'s dependencies.

### 10. `Path<T>`'s startup check is exact in both directions, and a newtype is checked as what it wraps

- **Design:** "A struct is checked by its field names, serde renames included; a tuple by its
  count; a scalar or newtype requires exactly one parameter."
- **Written:** a struct's fields must be exactly the route's parameters: a field the route lacks
  is refused, and so is a route parameter no field names. The error lists both sides. A newtype
  is checked as what it wraps: `UserId(u64)` takes one parameter, as designed, and `Wrapper(Params)`
  is checked as `Params`. `Option<T>` is checked as `T`. A unit, a map, a sequence and
  `deserialize_any` are unchecked.
- **Why:** "exactly", because the tuple rule is exact and a struct ignoring a parameter is the
  same mistake as a tuple too short. Recursing into a newtype gives the designed answer for a
  scalar newtype and a checked answer where "exactly one" would refuse a valid struct newtype.
- **Consequence:** an `Option` field the route never provides is refused at startup, though it
  would extract as `None`.

### 11. An empty body is `Missing` for `Json`, `Form` and `Multipart`

- **Design:** silent; `Option<P>` is `None` on `Missing`.
- **Written:** a body that has ended before it is read (`is_end_stream`), or a declared
  `Content-Length: 0`, is `Missing { param: "body" }`, whatever the media type, so
  `Option<Json<T>>` is `None` for a request carrying no body. `Bytes` extracts an empty body as
  empty `Bytes`, and `BodyStream` as an empty stream. A body already taken (by a guard reading
  `take_body`) is `Missing` for all five.

### 12. Media types, and the order of 415 and 413

- **Written:** `Json` accepts `application/json` and any `+json` structured-syntax suffix
  (RFC 6839), the essence compared case-insensitively without parameters; `Form` accepts
  `application/x-www-form-urlencoded`; `Multipart` needs `multipart/form-data` with a boundary.
  A body without `Content-Type` is 415. The media type is checked first, then `Content-Length`
  against the route's limit (413 before any byte is read), then the bytes as they arrive (413 the
  moment they pass the limit, whatever `Content-Length` said). A failed read is
  `Malformed { param: "body" }`.
- **`BodyStream` and `Multipart`:** a declared length over the limit is 413 at extraction; a
  stream that passes the limit while read ends with a boxed `ExtractError::TooLarge` and then
  `None`. For `Multipart` that error surfaces from `next_field` as multer's `StreamReadFailed`.

### 13. `Header<H>` and `LastEventId`

- **Written:** `Header<H>` reads every value of `H::name()`; none is `Missing` naming the
  header, and a failed `decode` is `Malformed` naming it. `LastEventId` reads the header's bytes
  as UTF-8, which admits an id with non-ASCII characters (the SSE spec's ids are any string
  without U+0000, CR or LF); bytes that are not UTF-8 are `Malformed`.

## Replies and rendering

### 14. Which bodies are wrapped in `Tracked`

- **Design:** "Every streaming answer is wrapped in `fw_transport::Tracked<S>`."
- **Written:** a body whose length is not known (`size_hint().exact()` is `None`, as for
  `Body::stream(s)`) is wrapped, in the `IntoReply` impls of `HttpBody` and of `Response`. A body
  of known length is written whole and is not wrapped, so an `on_stream_end` callback registered
  for one never runs. `Sse` wraps its own body (entry 17).
- **Why:** wrapping a known-length body would lose its exact size hint, and with it
  `Content-Length`, on every response.

### 15. Problem documents

- **Written:**
  - `title` is the RFC 9110 phrase, which RFC 9457 makes the title of an `about:blank` problem:
    "Content Too Large" for 413 and "Unprocessable Content" for 422, where the `http` crate still
    gives the pre-RFC 9110 phrases, and the `http` crate's phrase otherwise.
  - `details` is left out when empty.
  - `Retry-After` is written in delay-seconds, a fraction rounded up so a client never retries
    early, which agrees with the `seconds` that `Details`' JSON writes.
  - A 401 challenge that is not a valid header value, from configuration or from
    `CallError::unauthorized`, is logged at `warn` and `Bearer` is sent instead.
  - `draining` and `shed` answer problem documents of kind `Unavailable`, each with
    `Retry-After: <shed_retry_after>`; `draining` adds `Connection: close`.
  - If `Details` fails to serialize, the document or envelope is written without it and the
    failure logged at `warn`.

### 16. 413 and 415 from an `ExtractError`, and `Accept` on 415

- **Design:** "HTTP adds the exact statuses 413 and 415 from the variant."
- **Written:** `render_error` finds the `ExtractError` bare or as the source of a `CallError`
  (T's `Param::extract` boxes it as the latter). The status applies only while that `CallError`
  is still of kind `BadRequest`; an error handler that reshaped it to another kind is rendered by
  that kind. A 415 carries `Accept` with the media type the extractor expected, which RFC 9110
  §15.5.16 suggests.

### 17. SSE

- **Plan point, keep-alive:** a `: keepalive` comment is written whenever the period passes with
  nothing written, and the clock starts again at every event and every comment, so none is
  written while events flow. The timer is tokio's (`tokio::time::Sleep`), created on the body's
  first poll, so the body must be polled on a tokio runtime with time enabled. The app's `Timer`
  is not reachable from `HttpCx`. Each keep-alive frame is `: keepalive` followed by a blank line,
  which dispatches nothing.
- **Headers:** `Content-Type: text/event-stream; charset=utf-8` (the media type's registration
  allows only `utf-8` for `charset`) and `Cache-Control: no-cache`, which the design does not
  name; it keeps an intermediary from caching an event stream.
- **Encoding:** every field is written `name: value`. A reader drops exactly one space after the
  colon, so a value keeps a leading space of its own. Comment lines come first, then `event`, `id`,
  `retry` (milliseconds), then data. Data and comment text split at CR, LF and CRLF. The piece
  after a trailing terminator is kept, so `"a\n"` round-trips. Empty data is one `data: ` line,
  which a reader dispatches as `""`; an event without data has no `data` line.
- **The late path:** an `Err` item runs `ulo::dispatch_late` with the matched handler, the future
  boxed in the body. `Render(e)` writes `e`; `Ignored` is logged at `warn` and the original's
  `summary()` is written; `End`, like the stream returning `None`, writes the end event if one is
  set and completes. A written error event reports `StreamOutcome::CutOff(None)` on the execution
  first; `Tracked` reports `Completed` when the body later ends, which changes nothing, since
  the first report wins. On an execution cancelled with `Deadline` the error written is
  `Timeout`, as `render_error` does.
- **An `Sse` with no matched handler:** an error handler answering a miss could return one; its
  item errors are written as they are, with no error handlers run.
- **The `error` event's envelope** always carries `details`, an empty array when there are none,
  so its shape is fixed.

### 18. `Request::set_path` adds a missing leading `/`

- **Spine:** "rebuild the URI's path-and-query with `path` and the existing query".
- **Written:** as the spine says. A path without a leading `/` gets one, since an origin-form
  request target always starts with it (RFC 9112 §3.2.1). A path that does not parse fails with
  the `http::Error` and leaves the request unchanged. The doc of `Request::upgrade` now also
  says it is `None` on a request that asks for no upgrade, as HM's backend leaves it.

### 19. Crate-private additions

- `PatternError` and `RouteError` implement `Display` (and `PatternError` `Error`), so P can
  report them in a `Configure` error as they stand.
- `pattern::{normalize, split}`, `Pattern::{match_split, precedence}`, `RouteEntry`'s private
  `allow`, `HttpBody::tracked`, `render::timed_out` (which P's service uses for a route timeout),
  `EventName::error`.

## Requests to other areas

### P (`service.rs`, `server.rs`, `pre_dispatch.rs`)

1. **`HEAD` from `GET`:** `service.rs`'s `without_body` adds `Content-Length` from the body's
   exact size on every status. On a `GET` answering 204, that writes `Content-Length: 0` on a 204,
   which RFC 9110 §8.6 forbids, and likewise on 1xx and 304. Skip it when
   `parts.status.is_informational()` or the status is 204 or 304 (entry 7).
2. **`ScopePattern`:** it has private fields now, so it is built with `ScopePattern::parse`, as
   `pre_dispatch.rs` already does (entry 8).
3. **Route errors:** `Router::build` returns every failure as a `Vec<RouteError>`, each a full
   sentence through `Display`, naming handlers as `Controller::method`.

### CX (`crates/ulo`)

4. **`dispatch_late` must be `Send`:** the SSE body boxes `ulo::dispatch_late(..)` as a
   `BoxFuture<'static, LateOutcome>` from owned clones of the handler, the execution and the
   `HttpCx`, so the future `recover` returns must be `Send` for `T = Http`. P's `AppService::call`
   assumes the same of `dispatch`. `MountedHandler<Http>` must also be `Send + Sync`, since the
   body holds a clone.

### Coordinator

5. **Workspace dependency:** `serde_urlencoded` is no longer used by `ulo-http` (entry 9). The
   workspace entry can go unless another crate takes it.

### HM

6. **Pattern grammar:** confirmed in step (entry 3). No change requested.

**P's request H1, applied by the orchestrating session after H finished:** `HttpCx::head`'s doc now says it is
the head as the pre-dispatch stage left it, and that the seeded `Dep<RequestHead>` is the head as the client
sent it. Doc only. P's optional H2 (expose a 405 renderer) is left for after the first compile.
