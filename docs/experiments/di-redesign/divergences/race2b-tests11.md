# Divergences: race 2b, tests batch 11, unencodable replies, the RPC context parameter, the gRPC reply traits, the post-deadline buffer and the clippy pin

The twenty-eighth response, signed off 2026-10-07, settled five builds. A reply frame the link
cannot encode is answered `err` of kind `internal` under the same id and logged at `error` (F340),
where it had been read as a departed caller. `RpcCx` is a handler parameter, as the other three
contexts are (F339). The three public gRPC reply traits take `Into*` names, each named after its
one method. HTTP buffers an error handler's answer to a passed deadline up to a fixed 1 MiB, and a
buffered stream reports its end when the backend has written the copy. The `generated code lints`
job runs a pinned clippy.

Files added: `crates/ulo-http/tests/post_deadline_body.rs`, this file.
Files changed: `Cargo.lock`, `.github/workflows/ci.yml`, `crates/ulo-rpc/src/{codec.rs, link.rs,
dispatch.rs, client.rs, lib.rs, transport.rs}`, `crates/ulo-rpc-conformance/src/{lib.rs,
cases/app.rs, cases/errors.rs, cases/unary.rs}`, `crates/ulo-macro-lints/src/rpc.rs`,
`crates/ulo-grpc/src/{transport.rs, __private.rs, dispatch.rs, lib.rs}`,
`crates/ulo-http/{Cargo.toml, src/service.rs, src/limits.rs, src/server.rs, src/backend.rs}`,
`crates/ulo-http-hyper/tests/route_timeout.rs`, `docs/experiments/di-redesign/transports/DESIGN.md`
(the trait rename only), and F339, F340 and the new F342 in the workspace's `FRAMEWORK_GAPS.md`.
The tree is `a1fd6f52` plus this batch.

## The signatures

```rust
// ulo_rpc::Codec
pub fn encode_frame(self, frame: &Frame) -> Result<Bytes, FrameUnencodable>;   // was Result<Bytes, BoxError>

// ulo_rpc::link, re-exported at the root beside FrameTooLarge
#[derive(Debug)]
pub struct FrameUnencodable { pub codec: Codec, pub source: BoxError }
impl Display for FrameUnencodable;   // "the Cbor codec cannot encode the frame: <source>"
impl Error for FrameUnencodable;     // source() answers `source`

// ulo_rpc
impl FromCall<Rpc> for RpcCx;        // answers a clone of the call's context

// ulo_grpc, renamed; the implementations are unchanged
pub trait IntoGrpcReply: Send + 'static { type Message; fn into_grpc_reply(self) -> Reply; }            // was ReplyValue::into_grpc
pub trait IntoGrpcStream: Send + 'static { type Message; fn into_grpc_stream(self, cx: &GrpcCx) -> Reply; } // was ReplyStream::into_grpc
pub trait IntoGrpcItem: Send + 'static { type Message: prost::Message + Default + Send + 'static;
                                         fn into_grpc_item(self) -> Result<Self::Message, CallError>; } // was ReplyItem::into_item
```

Private to `ulo-http`'s service: `BUFFER_CAP` (`MB`), `Incomplete::Oversized`, `Buffered::read`
and its `Drop`, the `Read` struct, and `completed` taking the execution.

## Decisions

### 1. An encoding failure is a typed error from the codec

- **Written:** `Codec::encode_frame` wraps whatever its encoder returns in `FrameUnencodable`. Every
  link in the workspace encodes its reply frames with `encode_frame` and passes the error on with
  `?`, so the boxed error reaching the dispatcher is a `FrameUnencodable` with no change to any
  link. The dispatcher's `send` takes it into the `FrameTooLarge` arm: an `err` of kind `internal`
  under the same id, message "the reply could not be encoded", and
  `tracing::error!(error = %err, "a reply frame could not be encoded; the call is answered `internal`")`.
  `FrameTooLarge` keeps its silence; only the encoding failure logs.
- **Why the codec:** the distinction lives where the failure happens. A send error that is neither
  type still means a closed lane. A link encoding its frames some other way returns
  `FrameUnencodable` itself; the fields are public for that, as `FrameTooLarge`'s are.
- **`Display` carries the source, and `source()` answers it too,** as `StartupError` and `Redacted`
  do, so the `error` log names the encoder's complaint and a chain walker sees it once.
- **The follow-up `err` reaches the caller on every link.** On TCP and UDP the id mapping is
  forgotten only after a terminal frame is sent, and the failed `res` was never sent. The broker
  links release the call from their table before encoding, which removes the control lane's entry
  but not the reply path the `err` goes through.
- **The client side too:** `RpcClient` answers a `FrameUnencodable` from its own request's encoding
  `Internal`, where it had fallen to the catch-all `Unavailable` "the link refused the frame". The
  request's payload is encoded by the client from a `Serialize` value, so this arm is reached only
  by a link encoding its own frames wrongly. Not asked for; it follows from typing the error.

### 2. The scenarios: one per fix, on every link

- **`unencodable_reply_is_internal`:** a handler answers `Data::new(&[0xff])`, a lone byte that is
  no UTF-8, so no JSON text, and a CBOR break code with nothing open, so no CBOR item. The call must
  fail `internal` within the 5 s call timeout; the harness's own limit is twice that, so a server
  that sends nothing fails on the caller's `Timeout`, as the bite shows. On a link carrying streamed
  replies, a handler answers `Reply::Many` of one encoded `1u32` and then the same byte, and the
  stream must yield `Ok(1)` and then `internal`.
- **`handler_takes_its_context`:** a handler taking `cx: RpcCx` answers the pattern, the link's
  name and a header, each read through the context, and the scenario asserts all three against
  what it sent and `Link::NAME`.
- **Why scenarios:** both behaviours are the dispatcher's on every link, and the CBOR stamp runs
  the encoding failure under the second codec. `ulo-macro-lints` also takes `cx: RpcCx` on one
  handler, so the parameter's expansion is linted.

### 3. The gRPC rename

- **Written:** as signed off, in `ulo-grpc` and in DESIGN §6.1 and §6.2. Nothing in
  `ulo-codegen-tests`, `ulo-macro-lints` or `ulo-grpc-macros` names the traits: the generated code
  reaches them through `__private`'s probe. DESIGN §6.1's sentence on the traits now gives the
  `Into*` convention as the reason for the names.
- **Left as written:** `transports/DIVERGENCES_2B.md` and the earlier divergence logs, records of
  what was built under the old names.

### 4. HTTP: the buffer is capped at 1 MiB, and a buffered stream reports its end at the write

- **The cap:** `completed` refuses a body whose `size_hint().lower()` is already over 1 MiB before
  reading it, and a body whose data would pass 1 MiB as it is read, with `Incomplete::Oversized`.
  The service answers the canonical 504 and logs at `warn`, with the route and `limit`: "an error
  handler answered a timed-out call with a body larger than the buffer for a reply after the
  deadline; it was dropped at the deadline". Exactly 1 MiB is buffered.
- **One cap for every body:** a body of known length goes through the same check, a `Full` of
  2 MiB answering 504 too. The rule as signed off names no exception, and a body reporting an exact
  length may still produce its bytes lazily.
- **The stream end:** a body of unknown length, which is the condition under which `HttpBody`'s
  `tracked` wraps a reply in `Tracked`, is kept inside the `Buffered` copy with its execution after
  `completed` reads it to its end. `Buffered`'s `Drop` reports `Completed` when every frame was
  handed to the backend, `CutOff(exec.cancel_reason())` otherwise, and then drops the original
  body. `report_stream_end` keeps the first report, so the `Tracked` stream, which read its own
  end, adds nothing. A body of known length was never tracked and reports nothing.
- **Written means handed over:** `Buffered` ends after its last frame, so the backend learns
  `is_end_stream` with that frame and drops the body once it has written it; that drop reports
  `Completed`, which is when `Tracked` and `ExecBody` take a body as written.
- **The reason on `CutOff`:** a peer leaving fires `Disconnected`, but the route timeout has
  already cancelled the execution with `Deadline` and the first reason stays, so the callback
  receives `CutOff(Some(Deadline))`. Every stream on this path carries that reason.
- **A body not tracked, of unknown length:** an error handler writing
  `Response::new(HttpBody::stream(..))` without `into_reply` has no `Tracked` inside, and its
  buffered copy now reports its end where it reported nothing before. `on_stream_end` then runs
  for a stream reply on this path whether or not the handler wrapped it.

### 5. Two existing route-timeout tests moved under the cap

- **What failed:** `an_event_stream_answered_after_the_route_timeout_is_replaced_by_504_when_the_grace_ends`
  answered after 259 ms rather than past the 700 ms deadline and grace: its `stream::repeat_with`
  `Sse` is always ready and passed 1 MiB within about 60 ms of the deadline.
- **Written:** its `Sse` ticks every 20 ms, still open with a few hundred bytes when the grace runs
  out, which is the test's subject. The property the always-ready stream also held, that reading a
  body which never returns `Pending` yields to the runtime so the grace's timer can fire on a
  current-thread runtime, moved to a new test, `/spinning`: a body of empty data frames, always
  ready and never ending, which no cap reaches. With `FRAMES_PER_POLL` at `usize::MAX` it hangs.

### 6. Where the post-deadline tests sit

- **Through hyper:** the over-cap answer, in `route_timeout.rs`, with the `warn` and the time: the
  504 arrives before the grace runs out, so the cap ended it.
- **Through `AppService::call`:** the stream-end outcome and the cap's boundary, in
  `crates/ulo-http/tests/post_deadline_body.rs`, whose backend is the test. A peer leaving before
  the buffered copy is written cannot be produced over a socket: the copy is ready when `completed`
  returns, and hyper writes it into the socket buffer at once. A backend that drops the body
  unwritten is what a peer leaving is, by the `Backend` contract, so the test plays that backend.
  `ulo-http` gains dev-dependencies on `ulo-tokio` and `tokio`.

### 7. The clippy pin

- **Written:** `dtolnay/rust-toolchain@1.98.1` with `clippy` in the `generated code lints` job, the
  stable release in use, and the version the `macro diagnostics` job pins. The comment says why and
  that the pin is bumped in a commit of its own, fixing whatever the new release trips. The
  `macro diagnostics` comment called its pin the only one not read from a manifest, and now names
  the second.

## Unrelated gaps found

- **F342:** one run of the Redis conformance suite failed starting its server, and the runner's
  output filter dropped the reason; eight further runs passed. See Verification.

## Left for the transports DESIGN fold

These DESIGN statements describe the tree before this batch and are not edited here, beyond the
rename of §6.1 and §6.2: §1's crate table row for `fw-macro-lints` and its paragraph on generated
code (lines 25 and 46), and decision 40 (line 1329), "on stable"; §5.1's reply paragraph (line
837), "`RpcCx` implements no `FromCall<Rpc>`"; §5.3's link rules (line 938), the encoding failure
read as a closed lane, and the SPI block (line 935) without `FrameUnencodable`; §3's cancellation
paragraph (line 495), the post-deadline buffer without a cap.

## Needs sign-off

### S1. `FrameUnencodable` and `encode_frame`'s error type

Decision 1: a public error type in the link SPI, and `Codec::encode_frame` answering it instead of
`BoxError`. A link calling `encode_frame` and boxing its error keeps compiling.

### S2. The client answers its own encoding failure `Internal`

Decision 1's last point: a change on the request side the response did not ask for, and pinned by
no test.

### S3. The over-cap `warn` is its own line

Decision 4: "counts as streamed" read as the same 504 with a `warn` of its own text, naming the
buffer rather than a stream, since a known-length body over the cap is no stream.

### S4. The cap applies to a body of known length

Decision 4: a known-length body over 1 MiB answers 504 like a stream.

### S5. An untracked stream on this path now reports its end

Decision 4's last point.

## Verification

Each new test against the change undone, every file restored by `shasum -c`:

| Undone | Test | Result |
| --- | --- | --- |
| `send`'s arm without `err.is::<FrameUnencodable>()` | `unencodable_reply_is_internal`, TCP and the CBOR stamp | both fail after 5 s: "expected an error of kind Internal, got: Err(RpcError { kind: Timeout, message: \"the call timed out\", .. })" |
| the same, the scenario's unary assertion skipped | `unencodable_reply_is_internal`, TCP | fails after 10 s: "expected the item `1` and then an `internal` error, got: [Ok(1), Err(RpcError { kind: Timeout, .. })]" |
| `impl FromCall<Rpc> for RpcCx` removed | the conformance crate | E0277 "`RpcCx` cannot be built from a `Rpc` call" at both `cx: RpcCx` parameters |
| `Buffered` keeps no read stream, the body dropped in `completed` | the two stream-end tests | both fail at "the stream's end was reported before the backend wrote the buffered body" |
| `BUFFER_CAP` at `u64::MAX` | `a_body_one_byte_over_the_cap_answers_504`; the hyper over-cap test | the first answers 503; the second "assertion `left == right` failed: body of 1114112 bytes" |
| the cap's `>` as `>=` | `a_body_of_exactly_the_cap_is_written_with_its_length` | fails |
| `FRAMES_PER_POLL` at `usize::MAX` | `an_always_ready_body_..._when_the_grace_ends` | hangs; the test binary was killed by a 30 s alarm |

The pinned job's command, `cargo +1.98.1 clippy -p ulo-macro-lints --all-targets --no-deps --locked
-- -D warnings`, passes.

Three consecutive runs per target on stable, one suite at a time, test time as libtest reports it;
the brokers with `--features integration`:

| Target | Run 1 | Run 2 | Run 3 | Result |
| --- | --- | --- | --- | --- |
| `ulo-rpc-tcp` conformance | 1.31 s | 1.31 s | 1.31 s | 24 passed |
| `ulo-rpc-tcp` conformance_cbor | 1.31 s | 1.31 s | 1.31 s | 24 passed |
| `ulo-rpc-tcp` deadline_answers | 0.21 s | 0.20 s | 0.20 s | 2 passed |
| `ulo-rpc-udp` conformance | 1.31 s | 1.31 s | 1.31 s | 23 passed, 1 ignored |
| `ulo-rpc-nats` | 5.55 s | 3.03 s | 5.16 s | 24 passed |
| `ulo-rpc-redis` | 12.08 s | 5.08 s | 5.15 s | run 1: 23 passed, 1 failed; runs 2 and 3: 24 passed |
| `ulo-rpc-mqtt` | 5.29 s | 5.16 s | 5.12 s | 24 passed |
| `ulo-rpc-rabbitmq` | 27.09 s | 29.43 s | 29.12 s | 24 passed |
| `ulo-rpc-kafka` | 26.85 s | 24.29 s | 22.41 s | 24 passed |
| `ulo-http-hyper` conformance | 0.32 s | 0.31 s | 0.31 s | 36 passed, 2 ignored |
| `ulo-http-hyper` route_timeout | 0.71 s | 0.70 s | 0.70 s | 5 passed |
| `ulo-http` post_deadline_body | 0.06 s | 0.05 s | 0.05 s | 4 passed |
| `ulo-http-axum` | 0.32 s | 0.31 s | 0.31 s | 38 passed |
| `ulo-http-actix` | 1.03 s | 1.01 s | 1.02 s | 34 passed, 4 ignored |
| `ulo-http-actix` conformance_http2 | 4.04 s | 4.02 s | 4.02 s | 38 passed |
| `ulo-http-poem` | 0.74 s | 0.73 s | 0.72 s | 38 passed |
| `ulo-http-rocket` | 4.03 s | 4.01 s | 4.01 s | 38 passed |
| `ulo-http-salvo` | 0.32 s | 0.31 s | 0.31 s | 39 passed |
| `ulo-codegen-tests` replies | 0.01 s | 0.00 s | 0.00 s | 12 passed |
| `ulo-codegen-tests` deadlines | 0.51 s | 0.51 s | 0.51 s | 10 passed |

Redis's first run failed `unhandled_event_is_acknowledged` at the server's startup panic
(`cases/app.rs:484`), in a scenario this batch did not touch and before anything it changed runs.
The runner kept only the `panicked at` line, so the bind error the message carries was lost; six
further runs one after another all passed (5.00 to 5.27 s). Filed as F342. The Docker engine
answered throughout; `postgres:18`, `redis:7`, `axllent/mailpit` and `chrislusf/seaweedfs`, started
outside this batch, ran throughout.

- `cargo check --workspace --all-targets --all-features` on stable, with
  `CFLAGS=-I/opt/homebrew/opt/openssl/include LDFLAGS=-L/opt/homebrew/opt/openssl/lib`, and
  `cargo +1.88 check --workspace --all-targets --exclude ulo-http-salvo --exclude
  ulo-graphql-async-graphql`, each print the same 17 warnings, all in `crates/ulo/src`, the set
  listed in `batch2a-cfgattr.md`.
- `cargo test --workspace --no-fail-fast`, with the OpenSSL flags: 413 passed, 0 failed, 64
  ignored: batch 10's 401 plus the two new scenarios on each of the TCP, CBOR and UDP stamps, the
  four `post_deadline_body` tests and the two new `route_timeout` tests.
- `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --lib` over `ulo-rpc`, `ulo-grpc`, `ulo-http`,
  `ulo-rpc-conformance` and `ulo-macro-lints`: no warning. With `--document-private-items`,
  `ulo-rpc` and `ulo-grpc` are clean; `ulo-http` fails on the redundant explicit link target batch
  10 recorded, now at `service.rs:48`, one line down.
