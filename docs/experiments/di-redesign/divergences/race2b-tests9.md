# Divergences: race 2b, tests batch 9, a stream after a deadline, and RabbitMQ's suite time

The twenty-sixth response settled F334: after a passed deadline, an error handler's answer is
delivered only as a single reply produced within `timeout_grace`, and a streamed answer is ended at
once with the transport's canonical timeout, logged at `warn` as "an error handler answered a
timed-out call with a stream; the stream was ended at the deadline". RPC already ended such a
stream and now logs it. gRPC wrote it as returned and now ends it. HTTP did not already match:
an `Sse` an error handler answered a route timeout with ran for as long as its stream did, and it
is now replaced by the canonical 504. Each transport is pinned through a real client. RabbitMQ's
suite time was traced to its schedule and to host noise; no scenario grew, and nothing was fixed.

Files added: `crates/ulo-rpc-tcp/tests/deadline_answers.rs`,
`crates/ulo-http-hyper/tests/route_timeout.rs`, this file.
Files changed: `Cargo.lock`, `crates/ulo-rpc/src/dispatch.rs`, `crates/ulo-grpc/src/dispatch.rs`,
`crates/ulo-http/src/{service.rs, limits.rs}`, `crates/ulo-rpc-tcp/Cargo.toml`,
`crates/ulo-http-hyper/Cargo.toml`, `crates/ulo-codegen-tests/{Cargo.toml, proto/probe.proto,
tests/deadlines.rs, tests/support/mod.rs}`, and F334, F336 and F337 in the workspace's
`FRAMEWORK_GAPS.md`. The tree is `0782f73b` plus this batch.

## The signatures

No public signature changes. The new items are private to `ulo-grpc`'s dispatcher:

```rust
// crates/ulo-grpc/src/dispatch.rs
async fn within<F: Future>(fut: F, grace: Option<&mut BoxFuture<'static, ()>>) -> Option<F::Output>;
async fn single(reply: Reply, grace: Option<&mut BoxFuture<'static, ()>>) -> Result<Response, ()>;
fn past_first_message(data: &[u8]) -> bool;
struct Buffered { data: Option<Bytes>, trailers: Option<HeaderMap> }   // impl http_body::Body
```

The test dev-dependencies gained: `ulo-transport` on `ulo-rpc-tcp` (for `Tracked`, `CallError`),
`tracing` on `ulo-codegen-tests`, `ulo-tokio`, `futures-util` and tokio's `net`, `io-util` and
`time` on `ulo-http-hyper`. `Cargo.lock` gains four lines, all edges to packages already locked.
`probe.proto`'s `Clock` service gains `StallAnswered`, `StallStreamed` and `StallTrickled`.

## What each transport does after a passed deadline

| Transport | Single reply within the grace | Streamed answer | `warn` fields |
| --- | --- | --- | --- |
| RPC | `res` as answered | stream dropped unread; `err` of kind `timeout` | `pattern`, `handler` |
| gRPC | the reply as answered, its body read whole then written | trailers-only DEADLINE_EXCEEDED | `path` |
| HTTP | the response as answered | the canonical 504 problem document | `route` |

What counts as streamed, per transport:

- **RPC:** `Reply::Many`.
- **gRPC:** a reply whose body runs past its first length-prefixed message, or is still open when
  the grace runs out (decisions 1 and 2). A trailers-only status is single.
- **HTTP:** a response whose body has no exact `size_hint` (decision 4).

## Decisions

### 1. gRPC reads a stream off the wire, not off the reply's construction

- **Written:** after the error handlers answer `Ok(reply)`, `single` polls the body under the rest
  of the grace, appending its data. A byte after the first message's length prefix and payload
  makes it a stream; trailers or the body's end make it single, and the bytes read are written as
  one data frame followed by the trailers.
- **Why:** `ulo_grpc::Reply` is an erased `http::Response<tonic::body::Body>`. No public conversion
  builds one from a message or a stream (F336), so an error handler answering with either writes
  the body by hand with tonic's `EncodeBody`, which carries no mark of its shape. On the wire a
  server stream of one message and a unary reply are the same bytes, so the wire is where gRPC can
  tell them apart.
- **Alternative:** a marker extension set by the dispatcher's own `encode_stream`. It decides at
  once, and a hand-written body, the only one an error handler can write through public API, never
  carries it.

### 2. gRPC: a body still open at the end of the grace is a stream

- **Written:** the grace that bounded the error handlers bounds the body too, one sleep for both.
  A body with no second message and no end when it runs out answers DEADLINE_EXCEEDED and logs the
  `warn`. Under `Bound::Unbounded` the body is read until its end or its second message.
- **Consequence:** a stream whose items come readily is ended at its second message, within the
  poll that reads it. One that yields an item and then waits is ended at the grace's end, not at
  once. Nothing of either is written.
- **Also:** a body failing while it is read answers DEADLINE_EXCEEDED, logged at `debug`.

### 3. gRPC's refusal is the unclaimed path's answer

- **Written:** `status_response(status::deadline_exceeded())`, trailers-only, the reply's metadata
  dropped with its body.
- **Why:** the same response the call gives when no error handler claims the `Timeout`. RPC writes
  nothing of the stream either.

### 4. HTTP's stream is a body of unknown length

- **Written:** `response.body().size_hint().exact().is_none()` on the expired path answers
  `render::problem(&render::timed_out(), ..)`, dropping the headers the error handlers wrote, as
  the grace running out does.
- **Why:** `HttpBody::tracked` already draws the line there ("a body of known length is written
  whole"). An `Sse`, `Body::stream` and any `StreamBody` report no exact length; `HttpBody::from`,
  `empty` and every problem document do.
- **Consequence:** a single payload written through `Body::stream` is refused as a stream.

### 5. HTTP did not already end it

- **Found:** run against `0782f73b`'s `service.rs`, the SSE test failed with "the response to `GET
  /streamed` did not end within 5s". `TimedBody`, the moved-in sleep, wraps a response the pipeline
  answered before the timeout. The expired path returned the error handlers' response unwrapped,
  and `SseBody` does not poll the execution's cancellation, so an endless stream ran until the
  client left.

### 6. RPC answers the expired stream in its own arm

- **Written:** `Outcome::Expired(Some(Ok(Reply::Many(_))))` drops the stream, logs, and sends
  `timed_out()`, rather than reaching `stream_reply` and ending at its first poll.
- **Changed behaviour:** a link that carries no streamed reply answered this case `internal` ("the
  {link} link carries no streamed reply"), `stream_reply`'s capability check running first. It
  now answers `timeout`, as every other link does.

### 7. The tests capture tracing with a subscriber of their own

- **Written:** `Capture`, a `tracing::Subscriber` keeping each `warn` and `error` event's message
  and fields, installed with `tracing::subscriber::set_default` on a current-thread runtime, where
  the server's tasks run on the test's thread. One copy in each of the three test locations.
- **Why:** no workspace member depends on `tracing-subscriber` (it is locked through `loom`), and
  a thread-local default keeps one test's events out of another's.

## RabbitMQ's suite time

Batch 8 recorded 18.07, 19.82 and 14.04 s against batch 7's 11.60 to 13.17 s. Batch 8 changed
nothing in `crates/ulo-rpc-rabbitmq/src`; its `tests/conformance.rs` gained
`client_connections`, and the suite gained the `client_close` scenario. `Client::close` now calls
`Client::closed` and discards its report, the same `App::close` as before. No RabbitMQ close or
drain path changed.

Both trees' test binaries were built (`0782f73b` in place, `84bd5bcb` extracted with `git archive`
and built into a separate target directory) and each run nine times under `RUSTC_BOOTSTRAP=1 ..
-Z unstable-options --report-time`, alternating trees, the last six runs 20 s apart. `0782f73b`
was also run three times with `--skip client_close`. The host has 10 cores, so libtest runs 10
scenarios at once, starting them in name order.

Suite time, as libtest reports it:

| Tree | Runs | Median | Range |
| --- | --- | --- | --- |
| `84bd5bcb` (batch 7, 20 scenarios) | 13.08, 12.70, 11.63, 12.74, 13.16, 13.16, 12.62, 15.02, 12.68 | 12.74 s | 11.63–15.02 s |
| `0782f73b` (batch 8, 21 scenarios) | 12.09, 13.45, 19.95, 13.61, 13.38, 15.92, 11.95, 13.88, 14.37 | 13.61 s | 11.95–19.95 s |
| `0782f73b`, `--skip client_close` | 13.29, 14.64, 12.63 | 13.29 s | 12.63–14.64 s |

Per scenario, median of nine runs:

| Scenario | `84bd5bcb` | `0782f73b` |
| --- | --- | --- |
| `bidi_stream` | 4.62 s | 5.25 s |
| `binary_payload` | 5.28 s | 5.80 s |
| `cancel_mid_stream` | 5.34 s | 5.20 s |
| `client_close` | — | 5.69 s |
| `client_stream` | 4.88 s | 4.64 s |
| `client_timeout_is_timeout` | 4.98 s | 5.08 s |
| `deadline_ms_fires_deadline` | 5.09 s | 5.92 s |
| `domain_error_envelope` | 4.54 s | 5.13 s |
| `drain` | 6.57 s | 7.21 s |
| `event_reaches_its_handler` | 6.10 s | 5.88 s |
| `guard_refusal_is_forbidden` | 4.86 s | 4.51 s |
| `headers_reach_call_headers` | 4.65 s | 4.59 s |
| `oversized_payload` | 5.51 s | 5.44 s |
| `panic_is_internal` | 4.55 s | 4.94 s |
| `recovery_after_disrupt` | 8.20 s | 8.56 s |
| `server_stream_in_order` | 4.36 s | 4.87 s |
| `two_instances` | 5.93 s | 5.34 s |
| `unary_round_trip` | 4.55 s | 4.25 s |
| `undecodable_payload_is_bad_request` | 4.57 s | 4.25 s |
| `unhandled_event_is_acknowledged` | 4.98 s | 4.36 s |
| `unhandled_pattern` | 3.57 s | 3.10 s |

What the runs show:

- **No scenario grew.** The medians move by under a second each way. The rises sit in the
  scenarios that start first, which carried the noisy runs below.
- **The suite's time is `recovery_after_disrupt`'s end.** It finished last in all 21 runs. Its body
  is the longest, about 8 s, and it starts in the second group of ten, when a slot frees. Its start,
  the suite time less its own: median 4.67 s on `84bd5bcb`, 5.45 s on `0782f73b`.
- **`client_close` delays that start by one slot.** It sorts fourth, so on `0782f73b`
  `recovery_after_disrupt` is the fifth scenario of the second group rather than the fourth, and
  waits for a fifth first-group scenario to finish. That is the cost of a 21st scenario, about
  0.8 s at the median, and a correct one.
- **The long runs are host noise.** In the 19.95 s run all ten first-group scenarios took
  10.0–13.6 s, `client_stream` 10.98 s among them (2.66–6.75 s in the other 20 runs), while the
  second group ran at
  its usual times: the ten containers booted slowly together. The 15.92 s run is the same shape,
  milder (first-group median 6.37 s), and the batch 7 tree did it once too (15.02 s, first-group
  median 5.90 s). Swap stood at 12.4–13.6 GB of 14 GB through these runs; batch 8 reports 13.0 to
  13.8 GB through its own, where its 18 and 19.8 s runs fall.

## Needs sign-off

### S1. gRPC tells a stream by its bytes

Decisions 1 and 2. The alternative is a marker on the dispatcher's own streams, which no reply an
error handler can build carries.

### S2. HTTP tells a stream by its unknown length

Decision 4, including a single payload written through `Body::stream` being refused.

### S3. RPC on a link without streamed replies answers `timeout`

Decision 6's changed behaviour.

### S4. F336 and F337 filed

F336: no RPC or gRPC error handler can build a reply value through public API; the gRPC tests write
tonic's `EncodeBody` by hand. F337: clippy's `diverging_sub_expression` on every RPC handler, from
the macro's reply probe; a one-line `allow`, left unbuilt as outside this batch.

### S5. DESIGN text this falsifies, left for the fold

- §2.4, the deadline paragraph: the grace covers a single reply; a streamed answer is ended with
  the canonical timeout and logged.
- §3.6, the cancellation bullet: "Their answer is the response" gains the 504 for a body of
  unknown length.
- §5.2, the deadline paragraph: "a stream the error handlers answer after the deadline ends at its
  first poll the same way" becomes the stream dropped unread, `err` of kind `timeout`, and the log
  line.
- §6.2, the deadlines bullet: "a stream the error handlers answer after the deadline is written as
  they returned it" becomes decisions 1 to 3.

## Not covered

- **An RPC event's error handler answering a stream.** The event is acknowledged and the stream
  dropped unread, as before; nothing is delivered either way, and no line is logged.
- **The other six RPC links and the other HTTP hosts.** The change sits in `ulo-rpc`'s dispatcher
  and in `AppService`, which every link and every backend or embedding share; the new tests run on
  TCP and on the hyper backend.
- **A compressed gRPC reply.** `past_first_message` reads the length prefix whatever the
  compressed flag says; not exercised.

## Verification

Against known violations, each change temporarily made, run, and the file restored and confirmed
byte-identical by `shasum -c`:

| Violation | Run | Failure |
| --- | --- | --- |
| gRPC dispatcher as `0782f73b` has it | `deadlines` | `an_error_handler_answering_a_stream_is_ended_at_once_with_deadline_exceeded` and `an_error_handler_reply_still_open_at_the_end_of_the_grace_is_ended_with_deadline_exceeded`: "expected the call to fail before any item, got a stream whose first item is Some(Ok(Tick { n: 1 }))"; 8 passed |
| gRPC refusing any body with data | `deadlines` | `an_error_handler_answering_one_message_within_the_grace_is_the_reply` fails its `Ok(7)`; the open-body test answers before the grace ends; 8 passed |
| HTTP service as `0782f73b` has it | `route_timeout` | the SSE test: "the response to `GET /streamed` did not end within 5s"; 1 passed |
| RPC dispatcher as `0782f73b` has it | `deadline_answers` | the stream test: "the server logged no warning naming the pattern: []"; the `timeout` arrived; 1 passed |
| RPC ending the expired stream with `end` | `deadline_answers` | the stream test: "expected the call to end with the server's `timeout`, got None"; 1 passed |

Three consecutive runs per target on stable, one suite at a time, test time as libtest reports it;
the brokers with `--features integration`:

| Target | Run 1 | Run 2 | Run 3 | Result |
| --- | --- | --- | --- | --- |
| `ulo-rpc-tcp` conformance | 1.31 s | 1.31 s | 1.31 s | 21 passed |
| `ulo-rpc-tcp` `deadline_answers` | 0.21 s | 0.20 s | 0.20 s | 2 passed |
| `ulo-rpc-udp` conformance | 1.31 s | 1.30 s | 1.30 s | 20 passed, 1 ignored |
| `ulo-rpc-nats` | 2.78 s | 2.61 s | 2.52 s | 21 passed |
| `ulo-rpc-redis` | 4.90 s | 4.71 s | 4.76 s | 21 passed |
| `ulo-rpc-mqtt` | 5.04 s | 4.63 s | 4.82 s | 21 passed |
| `ulo-rpc-rabbitmq` | 12.77 s | 12.97 s | 13.14 s | 21 passed |
| `ulo-rpc-kafka` | 26.13 s | 23.82 s | 21.61 s | 21 passed |
| `ulo-codegen-tests` `deadlines` | 0.51 s | 0.53 s | 0.51 s | 10 passed |
| `ulo-http-hyper` `route_timeout` | 0.21 s | 0.21 s | 0.20 s | 2 passed |

NATS's three are runs 4 to 6. Its first run failed `client_timeout_is_timeout` after 77.60 s at
`crates/ulo-rpc-conformance/src/cases/app.rs:394`, "the conformance server did not start", the
other 20 passing; runs 2 and 3 passed (2.35 s, 2.33 s). The run's output was filtered to its first
three failure lines, so the bind error that followed the message was not kept. The scenario fails
in the server's start, before any call is made, and this batch changes nothing that runs there.
Not reproduced in five further runs.

Every test target of `ulo-codegen-tests` and `ulo-http-hyper`, and the conformance suites of
`ulo-http-axum` (36 passed), `ulo-http-actix` (32 passed, 4 ignored), `ulo-http-poem` (36),
`ulo-http-rocket` (36) and `ulo-http-salvo` (37), passed three runs each.

- `cargo check --workspace --all-targets --all-features` on stable, with
  `CFLAGS=-I/opt/homebrew/opt/openssl/include LDFLAGS=-L/opt/homebrew/opt/openssl/lib`, and
  `cargo +1.88 check --workspace --all-targets --exclude ulo-http-salvo --exclude
  ulo-graphql-async-graphql`, each print the same 17 warnings, all in `crates/ulo/src`, the set
  listed in `batch2a-cfgattr.md`.
- `cargo test --workspace --no-fail-fast`, with the OpenSSL flags: 362 passed, 0 failed, 62
  ignored, 670 s wall time: batch 8's 355 plus the seven tests above.
- `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --lib` over `ulo-grpc`, `ulo-http` and `ulo-rpc`:
  no warning; with `--document-private-items` on `ulo-grpc`, none in `dispatch.rs`.
- `cargo clippy` over the touched crates: nothing new in the changed source files. The new RPC test
  carries F337's four warnings.

Swap stood at 12.1 to 13.6 GB of 14 GB through the broker runs; `postgres:18` and `redis:7`,
started outside this batch, ran throughout. The Docker engine answered throughout.
