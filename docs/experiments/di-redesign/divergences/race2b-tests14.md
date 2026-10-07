# Divergences: race 2b, tests batch 14, the service owns stream tracking, bodiless answers report `Completed`, a host bound on conformance parallelism, the gRPC client's down endpoint

The thirty-first response, signed off 2026-10-07, settled four builds. The HTTP service is the one
place that wraps a streaming body in `Tracked`; `into_reply` and `Sse` no longer do. A stream the
backend never writes, on a `HEAD` answer or a 1xx, 204 or 304, reports `Completed`, the status
case with a `warn`. Both conformance crates read `ULO_CONFORMANCE_PARALLEL` as a host bound. The
gRPC client test that wants an endpoint down no longer probes a port and releases it (F346); the
listener the response prescribed does not produce the failure the test asserts, so the test aims at
port 0 instead (S5).

Files changed: `crates/ulo-http/src/{body.rs, response.rs, service.rs, sse.rs}`,
`crates/ulo-http/tests/{stream_end.rs, post_deadline_body.rs}`,
`crates/ulo-rpc-conformance/src/lib.rs`, `crates/ulo-http-conformance/src/lib.rs`, `Makefile`,
`crates/ulo-codegen-tests/tests/clients.rs`, and F346 in the workspace's `FRAMEWORK_GAPS.md`. The
tree is `bc8d9ec8` plus this batch.

## The signatures

```rust
// ulo_rpc_conformance (new)
pub const PARALLEL_VAR: &str = "ULO_CONFORMANCE_PARALLEL";

// ulo_http_conformance (new)
pub const PARALLEL_VAR: &str = "ULO_CONFORMANCE_PARALLEL";
```

`ulo_rpc_conformance::__private::Slots::hold(declared)` now applies the host bound too;
`ulo_http_conformance::__private::Slots` is new, with `hold()`. Both are macro ABI. No public item
of `ulo-http` changed. Private to it: `HttpBody::streams`, and `HttpBody::marked` narrowed from
`pub(crate)` to private, `tracked` and `rewrapped` being its only callers.

## Decisions

### 1. Where a body is wrapped, path by path

- **`into_reply` for `Response` and `HttpBody`:** the wrap is removed; both answer the body as
  given. Their docs no longer mention `Tracked`.
- **`Sse::into_reply`:** its own `Tracked` wrap and mark are removed too. The late path never
  depended on that wrap: `SseBody::late` reports `CutOff(None)` through `report_stream_end`
  directly, ahead of the `None` the stream then returns, and the first report stays. That call
  keeps a comment saying why it reports there. With the service's wrap the outcomes are those the
  `Sse` wrap gave: `Completed` at the end, `CutOff(None)` after an `error` event,
  `CutOff(Disconnected)` when the peer leaves. See S1.
- **The service's `respond`:** the one wrap, unchanged, skipped for a body the backend never
  writes (decision 2).
- **The post-deadline path keeps a wrap of its own.** `completed` now passes the error handlers'
  body through `tracked` before reading it, with a comment saying why: a body that path does not
  write, open at the grace's end, over the 1 MiB cap or failing, is dropped inside `expired`, before
  the service sees it, and would otherwise report nothing. Before this batch a stream built through
  `into_reply` reported `CutOff(Deadline)` there through `into_reply`'s wrap, and one built with
  `Response::new` reported nothing; both now report `CutOff(Deadline)`. A body written as the
  buffered copy reports through the copy, as before.
- **`HEAD` answered by the `GET` handler:** `found` no longer wraps before stripping; it reports
  `Completed` (decision 2).
- **`TimedBody`:** a body that reaches it unmarked is now wrapped by the service outside it,
  `Tracked(TimedBody(stream))` where `into_reply` used to give `TimedBody(Tracked(stream))`. Both
  read the cancel reason at their drop, after `ExecBody`'s, so the outcome is unchanged.
- **A stream discarded before the service sees it,** by an interceptor or a pre-dispatch entry
  answering something else, or by `WithHeaders` failing on a header after its inner reply was
  built, now reports nothing. Through `into_reply`'s wrap it reported `CutOff`, and that first
  report then shadowed the report of the body actually written. A callback registered for a
  discarded stream fires for whatever body replaces it, or not at all when that body has a known
  length. See S2.

### 2. A response that owes no body reports `Completed`

- **One function, `owes_nothing`,** called by `found` before it strips a `HEAD` answer's body,
  and by `respond` for a body it marks unwritten (method `HEAD`, or a status that forbids content).
  It reports `Completed` when the body is a stream, of unknown length or already tracked, and
  nothing for a body of known length, as such a body reports nothing when written.
- **The `warn`:** "a response carried a streaming body its status forbids; the body was discarded",
  field `status`, for 1xx, 204 and 304, logged inside the call span in `respond`. A `HEAD` answer
  carrying a 200's stream logs nothing; a `HEAD` answer with a 204 and a stream warns, since the
  status forbids the body whatever the method.
- **A `HEAD` handler's own stream** reports `Completed` without a log, as a `GET` handler's does on
  `HEAD`. The response named only the `GET` handler's case. See S3.
- **When it reports:** as the service answers, before the backend writes the head. An already
  tracked stream inside then reports `CutOff` on its drop and changes nothing. See S4.
- **The body stays in the response** for the backend to discard, as before, so nothing on the
  wire changes; `ExecBody` still marks it ended, so its drop fires no `Disconnected`.
- `forbids_body` names the 1xx/204/304 test that `respond` and `without_body` each spelled out.

### 3. `ULO_CONFORMANCE_PARALLEL`

- **RPC suite:** `Slots::hold` takes the broker's `PARALLEL` and applies the smaller of it and the
  variable, or whichever is set alone; neither set, no slot is taken, as before.
- **HTTP suite:** it has no broker and no declared bound, so the variable alone bounds it. One
  semaphore per stamped suite, both modes counted together, the permit held for the scenario.
- **Parsing, both crates:** read once per test binary. Unset or empty (after trimming) bounds
  nothing, so `ULO_CONFORMANCE_PARALLEL=` from a make variable left blank is not an error. Any other
  value that is not a positive integer fails every scenario through `startup_failed!`, naming the
  variable and the value. The parse is duplicated rather than shared: the two crates share no
  dependency where a test-only helper belongs.
- **Docs:** each crate's root doc and `PARALLEL_VAR`; `Broker::PARALLEL` says what one host's
  memory allows is the variable's to set, not the broker's. The Makefile has one local conformance
  target, `conformance-kafka`, and its comment documents the variable with an example; no target
  was added for the other brokers.
- **Unit tests** in `ulo-rpc-conformance`: `the_smaller_of_two_bounds_applies` and
  `either_bound_alone_applies`, over the private `bound`.

### 4. F346: the gRPC client's endpoint is port 0

- **The response's listener was built first and does not serve.** A listener on port 0 that
  closes each connection as it accepts it gets the call answered `Unknown`, "transport error",
  source `hyper::Error(Io, Kind(ConnectionReset))`, and a second call `Cancelled`. tonic answers
  `Unavailable` only for a failure during the TCP connect (`ConnectError`,
  `tonic-0.14.6/src/status.rs`); a connection that is accepted and then ends is a different
  failure path, so the test would have had to assert another code.
- **Built instead:** the client aims at `http://127.0.0.1:0`. Nothing can listen on port 0, since
  binding it chooses another port, so the connect fails every time and no process can take the port
  between a probe and the call: there is neither. On macOS the connect fails `EADDRNOTAVAIL`, which
  tonic answers `Unavailable`, "tcp connect error" (probed through the test); on Linux it is refused
  (probed with `redis-cli -p 0` in a throwaway `redis:7-alpine` container: "Connection refused").
  The assertion, `Err(Code::Unavailable)`, is unchanged. See S5.
- `tokio`'s `net` feature, added to the crate's dev-dependencies for the listener, was removed
  with it.

## The tests

- **`stream_end.rs`, ten tests** (five before). `a_head_answer_reports_its_unwritten_stream_cut_off`
  is now `a_head_answered_by_the_get_handler_reports_its_dropped_stream_completed`, expecting
  `Completed` and no `warn`. New: `a_head_handlers_stream_reports_completed` (an explicit `HEAD`
  route, no `warn`), `a_204_carrying_a_stream_reports_completed_and_warns` and
  `a_304_carrying_a_stream_reports_completed_and_warns` (the response dropped unread, as a backend
  drops a body it never writes; `Completed`, and a captured `warn` line starting with the message
  and `status=204` or `status=304`), `an_sse_stream_reports_completed_once_written` and
  `an_sse_stream_ending_in_an_error_event_reports_cut_off` (`CutOff(None)` though the stream then
  returns `None`). The `warn` is captured by a thread-default subscriber, the tests running on a
  current-thread runtime and calling the service on the test's task, as `route_timeout.rs` in
  `ulo-http-hyper` does.
- **`post_deadline_body.rs`:** `a_body_dropped_for_the_504_reports_its_stream_cut_off_by_the_deadline`
  (`/over-cap`, `CutOff(Some(Deadline))`); `WaitedOnTimeout` now registers `on_stream_end`.

## Left for the transports DESIGN fold

- §2.6 (line 313): "`into_reply` wraps a body of unknown length too and marks it, as `Sse` marks
  the body it builds" no longer holds; the mark is set by the service's own wrap alone. "the body a
  `HEAD` answered by the `GET` handler drops unwritten, which reports `CutOff`" is now `Completed`,
  with the 1xx/204/304 rule and its `warn`.
- §3's `HEAD` bullet (line 436): "a body of unknown length reporting its end as `CutOff`" is now
  `Completed`.
- §3's cancellation paragraph (line 495): "a stream an error handler hands over without
  `into_reply` ... reaches this path with no `Tracked` of its own and reports through the copy" now
  reads that every body of unknown length is wrapped as this path reads it; "the wrapper keeping the
  body's `Tracked` mark, so the service wraps it no further" holds only for a body already marked.
- §5.2's conformance paragraph (line 874) and decision 39 (line 1331): the host's
  `ULO_CONFORMANCE_PARALLEL` bound beside `Broker::PARALLEL`, and the HTTP suite's bound.

## Needs sign-off

### S1. `Sse` no longer wraps its own stream

Decision 1: the response named `Sse`'s late path as one that might need its own wrap. The late
path's report does not go through the wrap, so the wrap was a second place doing the service's job
and was removed; the late path's direct report stays, with its comment.

### S2. A discarded stream reports nothing

Decision 1: a consequence of removing `into_reply`'s wrap, not separately asked. The mark the
response asked to keep is set only by the service's own wrap: no public way exists to mark a body,
so a user's own `Tracked` inside `HttpBody::stream` is wrapped again and reports once, the first
report winning. A public constructor marking such a body would be new API and was not added.

### S3. A `HEAD` handler's stream reports `Completed` without a log

Decision 2: the response covered a `GET` handler answering `HEAD`; a `HEAD` handler mirroring a
`GET` answer's headers carries the same legitimate stream.

### S4. `Completed` is reported when the service answers

Decision 2: before the backend writes the head, rather than when it drops the body. A peer leaving
before the head is written still reads `Completed`; reporting at `ExecBody`'s drop would cover that
for `respond`'s case but not for `found`'s, which drops the body inside the pipeline.

### S5. Port 0 rather than an accept-and-close listener for F346

Decision 4: the prescribed listener makes the call fail `Unknown`, not the `Unavailable` the test
asserts. Port 0 meets every property the response asked for: deterministic, no probe, no release,
and the client's own connect failure. The alternative is the listener with the expected code
changed to `Unknown`, asserting a dropped connection rather than a down endpoint.

## Verification

Each new or changed test against its change undone, through a script that rewrote one line,
ran the target and wrote the file back; full output of every run kept:

| Undone | Result |
| --- | --- |
| `respond` wraps an unwritten body instead of calling `owes_nothing` | `a_head_handlers_stream_reports_completed`, the 204 and 304 tests fail: `left: Some(CutOff(None))`, `right: Some(Completed)` |
| `found`'s `owes_nothing` | `a_head_answered_by_the_get_handler_reports_its_dropped_stream_completed` fails: `left: None` |
| the `warn` | the 204 and 304 tests fail: "no `warn` that the body was discarded: []" |
| the `warn` on every unwritten stream | both `HEAD` tests fail: "a `HEAD` answer logged: [\"a response carried a streaming body its status forbids; the body was discarded status=200\"]" |
| `SseBody::late`'s report | `an_sse_stream_ending_in_an_error_event_reports_cut_off` fails: `left: Some(Completed)` |
| the service's wrap of a written body | five fail, `an_sse_stream_reports_completed_once_written` among them: `left: None` |
| `completed`'s wrap | `a_body_dropped_for_the_504_reports_its_stream_cut_off_by_the_deadline` fails: `left: None`, `right: Some(CutOff(Some(Deadline)))` |
| F346: the client aimed at a live server | `connect_does_no_network_io_and_the_first_call_finds_the_endpoint_down` fails: `left: Ok("a:ADA")`, `right: Err(Unavailable)` |

The override, against the TCP and hyper suites: `ULO_CONFORMANCE_PARALLEL=1` took the TCP suite
from 1.31 s to 5.26 s and the hyper suite from 0.31 s to 1.19 s, all passing;
`ULO_CONFORMANCE_PARALLEL=abc` failed all 24 TCP scenarios with "ULO_CONFORMANCE_PARALLEL is `abc`,
not a positive integer" (their 24 failure files then removed); `ULO_CONFORMANCE_PARALLEL=` passed
24 in 1.31 s.

Three consecutive runs per target on stable, one suite at a time, test time as libtest reports it;
the brokers with `--features integration --locked`.

| Target | Run 1 | Run 2 | Run 3 | Result |
| --- | --- | --- | --- | --- |
| `ulo-http` stream_end | 0.00 s | 0.00 s | 0.00 s | 10 passed |
| `ulo-http` post_deadline_body | 0.16 s | 0.15 s | 0.16 s | 7 passed |
| `ulo-http-hyper` conformance | 0.32 s | 0.31 s | 0.31 s | 36 passed, 2 ignored |
| `ulo-http-hyper` route_timeout | 0.71 s | 0.71 s | 0.71 s | 6 passed |
| `ulo-http-axum` | 0.48 s | 0.31 s | 0.33 s | 38 passed |
| `ulo-http-actix` | 1.03 s | 1.02 s | 1.03 s | 34 passed, 4 ignored |
| `ulo-http-actix` conformance_http2 (`--features conformance-http2`) | 4.04 s | 4.04 s | 4.04 s | 38 passed |
| `ulo-http-poem` | 0.73 s | 0.73 s | 0.73 s | 38 passed |
| `ulo-http-rocket` | 4.04 s | 4.03 s | 4.11 s | 38 passed |
| `ulo-http-salvo` | 0.32 s | 0.31 s | 0.31 s | 39 passed |
| `ulo-rpc-tcp` conformance | 1.31 s | 1.31 s | 1.31 s | 24 passed |
| `ulo-rpc-tcp` conformance_cbor | 1.31 s | 1.31 s | 1.31 s | 24 passed |
| `ulo-rpc-tcp` deadline_answers | 0.21 s | 0.20 s | 0.20 s | 2 passed |
| `ulo-rpc-udp` conformance | 1.31 s | 1.31 s | 1.31 s | 23 passed, 1 ignored |
| `ulo-codegen-tests` clients | 0.01 s | 0.00 s | 0.01 s | 5 passed |
| `ulo-rpc-nats` | 6.73 s | 6.83 s | 5.03 s | 24 passed |
| `ulo-rpc-redis` | 7.11 s | 6.56 s | 6.43 s | 24 passed |
| `ulo-rpc-mqtt` | 7.01 s | 5.91 s | 6.03 s | 24 passed |
| `ulo-rpc-rabbitmq`, libtest's default threads (10 CPUs), `ULO_CONFORMANCE_PARALLEL=6` | 122.16 s | 148.42 s | 142.96 s | 24 passed |
| `ulo-rpc-kafka` | 32.13 s | 34.64 s | 32.78 s | 24 passed |

RabbitMQ's runs were made with swap at 14,981–15,271 MiB used of 16,384 MiB and the user's four containers
running; batch 13's runs at the same parallelism without the bound failed 34 scenarios over three
runs on container startup timeouts. The Docker engine answered throughout; no container was left
behind, and the user's containers were not touched.

- `cargo check --workspace --all-targets --all-features` on stable, with
  `CFLAGS=-I/opt/homebrew/opt/openssl/include LDFLAGS=-L/opt/homebrew/opt/openssl/lib`, and
  `cargo +1.88 check --workspace --all-targets --exclude ulo-http-salvo --exclude
  ulo-graphql-async-graphql`, each print the same 17 warnings, all in `crates/ulo/src`, the set
  listed in `batch2a-cfgattr.md`.
- `cargo +1.98.1 clippy -p ulo-macro-lints --all-targets --no-deps --locked -- -D warnings` passes.
- `cargo test --workspace --no-fail-fast`, with the OpenSSL flags: 430 passed, 0 failed, 64
  ignored: batch 13's 422, the five new tests of `stream_end.rs`, the one of
  `post_deadline_body.rs` and the two unit tests of `ulo-rpc-conformance`.
- `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --lib -p ulo-http -p ulo-rpc-conformance -p
  ulo-http-conformance`: no warning. The same command failed on an unresolved link planted in
  `ulo-rpc-conformance` and passed again once it was removed.

### The port sweep

Every `TcpListener::bind`, `UdpSocket::bind` and `local_addr()` under `crates/` and `examples/`,
read in place, and a search for `free port`, `free_port` and `pick_free`, which finds the
`integration-tests` helpers and nothing in the workspace. The hosts and relays bind port 0 and keep
their listeners; `client_close.rs` in `ulo-rpc-udp` binds the client's released port to prove it
was released, which chooses no port; `relay::shadowed` binds a published container port to test
for a shadowing listener, which chooses none either. No probe-then-release port choice remains in
a built crate. `integration-tests`, which the workspace excludes, keeps the four sites F346 lists.
