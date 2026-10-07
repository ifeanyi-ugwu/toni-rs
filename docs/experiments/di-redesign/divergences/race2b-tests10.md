# Divergences: race 2b, tests batch 10, reply construction, completion within the grace, generated-code lints and startup reports

The twenty-seventh response, signed off 2026-10-07, settled four items. HTTP tells a single reply
after a deadline by completion within the grace: the error handlers' body is read under the rest
of the grace and written whole with its exact length when it ends in time, and a body still open
when the grace runs out answers the canonical 504. RPC and gRPC error handlers build a reply from a
value of their own through the call's context (F325, F336), pinned by a conformance scenario per
transport. Generated code is lint-clean under `clippy -D warnings` in a user's crate, held there by
a CI job over a crate that expands every macro (F337). Every startup path of both conformance
suites reports the underlying error (F338). Two gaps found on the way are filed as F339 and F340.

Files added: `crates/ulo-macro-lints/{Cargo.toml, build.rs, proto/lints.proto, src/lib.rs,
src/derives.rs, src/di.rs, src/enhancers.rs, src/http.rs, src/ws.rs, src/rpc.rs, src/grpc.rs}`,
`crates/ulo-rpc-tcp/tests/conformance_cbor.rs`, this file.
Files changed: `Cargo.toml`, `Cargo.lock`, `.github/workflows/ci.yml`,
`crates/ulo-http/src/{service.rs, limits.rs, server.rs, backend.rs}`,
`crates/ulo-http-hyper/tests/route_timeout.rs`, `crates/ulo-rpc/src/{transport.rs, __private.rs}`,
`crates/ulo-grpc/src/{transport.rs, __private.rs, dispatch.rs, lib.rs}`,
`crates/ulo-rpc-macros/src/message.rs`, `crates/ulo-build/src/markers.rs`,
`crates/ulo-rpc-conformance/src/{lib.rs, relay.rs, cases/app.rs, cases/errors.rs,
cases/delivery.rs}`, `crates/ulo-http-conformance/src/{lib.rs, reference.rs, app.rs,
cases/errors.rs}`, the conformance tests of `ulo-rpc-{tcp,udp,nats,redis,rabbitmq,mqtt,kafka}` and
`ulo-http-{axum,poem,rocket,salvo}`, `crates/ulo-http-actix/tests/common/mod.rs`,
`crates/ulo-rpc-tcp/tests/deadline_answers.rs`, `crates/ulo-codegen-tests/{proto/probe.proto,
tests/replies.rs, tests/deadlines.rs}`, and F325, F336, F337, F338, F339 and F340 in the
workspace's `FRAMEWORK_GAPS.md`. The tree is `e42cd467` plus this batch.

## The signatures

```rust
// ulo_rpc::RpcCx
pub fn codec(&self) -> Codec;                                             // was pub(crate)
pub fn reply<T: Serialize + ?Sized>(&self, value: &T) -> Result<Reply, IntoReplyError>;
pub fn reply_stream<S, U, E>(&self, items: S) -> Reply
where
    S: Stream<Item = Result<U, E>> + Send + 'static,
    U: Serialize + Send + 'static,
    E: Into<CallError> + Send + 'static;

// ulo_grpc::GrpcCx
pub fn reply<V: ReplyValue>(&self, value: V) -> Reply;
pub fn reply_stream<S: ReplyStream>(&self, stream: S) -> Reply;
pub fn reply_status(&self, status: tonic::Status) -> Reply;

// ulo_grpc, moved from the doc-hidden `__private` and re-exported at the root, unchanged
pub trait ReplyValue: Send + 'static { type Message; fn into_grpc(self) -> Reply; }
pub trait ReplyStream: Send + 'static { type Message; fn into_grpc(self, cx: &GrpcCx) -> Reply; }
pub trait ReplyItem: Send + 'static { type Message: prost::Message + Default + Send + 'static;
                                      fn into_item(self) -> Result<Self::Message, CallError>; }

// ulo_rpc_conformance and ulo_http_conformance
pub fn report(error: &(dyn Error + 'static)) -> String;
```

`ReplyValue` is implemented for every prost message and `Response<T>`, `ReplyStream` for every
stream of `ReplyItem`s and `Response<S>`, `ReplyItem` for `Result<T, E: Into<CallError>>`, as
before. An error handler writes:

```rust
// RPC
Ok(cx.reply(&Backorder { sku })?)
Ok(cx.reply_stream(stream::iter(items.map(Ok::<_, Refusal>))))
// gRPC
Ok(cx.reply(Response::new(pb::User::default()).metadata("x-cache", "miss")))
Ok(cx.reply_stream(ticks))
Ok(cx.reply_status(Status::unavailable("claimed")))
```

`RpcCx`'s private `encoded_stream` is what `reply_stream` and a handler's returned stream both go
through; a handler's `Serialize` answer now goes through `reply`. Private to `ulo-http`'s service:
`completed`, `Incomplete`, `Buffered` and `FRAMES_PER_POLL`, and `within` takes the grace by
`Option<&mut BoxFuture>`, so the error handlers and the body share one sleep.

## Decisions

### 1. HTTP: a single reply is a body that completes within the grace

- **Written:** after the error handlers answer, `completed` polls the response body under what
  remains of the grace, appending its data. Its end, or its trailers, make it a single reply,
  rewritten as `Buffered`: one data frame and the trailers, `size_hint` exact, so the backend
  writes `Content-Length`. A body still open when the grace runs out answers
  `render::problem(&render::timed_out(), ..)` and logs the `warn` naming the route; one failing
  while it is read answers the same 504, logged at `debug`. Under `Bound::Unbounded` the body is
  read until it ends.
- **Every body goes through it:** a body of known length ends at its first poll, and is rewritten
  like the rest. The shortcut is a body already at its end, which is passed on as it stands.
- **Yielding:** `completed` reads at most `FRAMES_PER_POLL` (32) ready frames per poll, then checks
  the grace and wakes itself. A body whose frames are always ready, `stream::repeat_with` among
  them, never returns `Pending`, and on a current-thread runtime the grace's timer fires only once
  the task yields; without the bound the read spins and buffers without end.
- **Consequences:** the data a body yields within the grace is held in memory, with no size bound
  beyond the grace's length; an endless `Sse` answered after a deadline costs whatever it produces
  in that window. A `Tracked` stream inside the body reports its end when `completed` reads it, not
  when the client receives the buffered copy.

### 2. RPC and gRPC error handlers build their replies through the context

- **Written:** `RpcCx::reply`, `reply_stream` and `codec`; `GrpcCx::reply`, `reply_stream` and
  `reply_status`. The handler path and the new methods share their encoders: RPC's `reply` and
  `encoded_stream`, gRPC's `encode_one`, `encode_stream` and `status_reply`.
- **Why the context:** RPC's encoding belongs to the link, and `RpcCx` holds the link's codec.
  gRPC's stream reply needs the call: an `Err` item takes the late path through the matched
  handler's error handlers, which `encode_stream` reads from the context. One spelling on both
  transports, `cx.reply(..)` and `cx.reply_stream(..)`, follows.
- **Why not constructors on `Reply`:** gRPC's `Reply` is a type alias for
  `http::Response<tonic::body::Body>`, which takes no inherent constructor in `ulo-grpc`.
- **Why not `IntoReply<Grpc>` impls:** `impl IntoReply<Grpc> for Response<T>` over a prost message
  and the same impl over a stream overlap (E0119), since a foreign type may implement both
  `prost::Message` and `Stream`, so a stream reply would need a second wrapper type beside the
  `Response<S>` a handler already writes. `GrpcCx::reply(impl Into<Response<T>>)` was probed and
  refused: passing a `Response<T>` is ambiguous between the reflexive `From` and `From<T> for
  Response<T>` (E0283).
- **Why public traits:** `GrpcCx::reply` takes what a unary handler may return, the message or a
  `Response<T>`, which is what `ReplyValue` already describes; making the three traits public names
  that set in the bound and in rustdoc instead of duplicating it. They are not sealed: a type a
  user implements `ReplyValue` for becomes a value a handler may return too, the reply probe's
  value arm being blanket over the trait.
- **`reply_status`:** answered with `Ok`, a status claims the error and ends the error handlers;
  returned as `Err`, the next handler is offered it. tonic's `Status::into_http` builds the same
  response; the method exists so the reply's three forms sit together.

### 3. The CBOR stamp of the TCP conformance suite

- **Written:** `crates/ulo-rpc-tcp/tests/conformance_cbor.rs`, the TCP suite with
  `Tcp::codec(Codec::Cbor)` on both sides. All 22 scenarios pass on it unchanged.
- **Why:** every other stamp runs a JSON link, so nothing showed a payload encoded with a codec
  other than the link's, which is F336's case: JSON bytes in a CBOR frame. With `RpcCx::reply`
  temporarily encoding with `Codec::Json`, the scenario passes on the JSON stamp and fails on this
  one.

### 4. The lint target is a dedicated crate

- **Written:** `crates/ulo-macro-lints`, `publish = false`, expanding every attribute macro
  (`injectable` on a struct and on an impl with `construct`, `module`, `routes` with `guards`,
  `interceptors`, `error_handlers` and `meta` at both tiers in all three spellings, the seven HTTP
  verbs, `ulo_ws::gateway` and `message`, `ulo_rpc::message` and `event`, `ulo_grpc::method` for
  each call shape), both derives (`Classify` on a struct and per variant, `Validate`), and a proto
  compiled by `ulo-build`. CI's job `generated code lints` runs
  `cargo clippy -p ulo-macro-lints --all-targets --no-deps --locked -- -D warnings` on `stable`.
- **Why not the existing crates:** `ulo-codegen-tests`, `ulo-ws`'s tests and the conformance
  suites' apps also carry hand-written code with clippy warnings of their own (`Slots` without
  `Default`, a large `Err` variant, a complex type), so `-D warnings` over them fails on code no
  user inherits. The dedicated crate holds bodies as small as the macros allow, and a warning there
  names generated code. `--no-deps` lints that crate alone; the framework's crates compile with
  their own warnings left as warnings.
- **Why `stable`:** a lint a new clippy adds reaches users on that release. The cost is a red job on
  a change unrelated to the macros when one arrives.
- **Found and fixed besides F337:** `ulo-build`'s marker module named as the service trips
  `clippy::module_inception` when the user's wrapper module carries the same name,
  `mod lints { include_proto!("lints.v1"); }` for the service `Lints`; the marker module now
  carries `#[allow(clippy::module_inception)]`. `ulo-codegen-tests` hit it with `mod bare` around a
  proto whose service is `Bare`. No other attribute macro's output trips a default lint: a
  `--no-deps` clippy over the whole workspace with `--all-targets` reports nothing else at a macro
  call site.

### 5. Startup reports go through one chain reporter

- **Written:** `report` walks `source()` and joins each cause's text with `: `, skipping one the text
  already carries, since `StartupError` and `Redacted` already print their chain. Every startup
  path in both suites panics with it: the broker containers' start and port mapping, the TCP and UDP
  probe sockets, the relay's bind and address, the server's `listen`, the client's wiring and
  connect, the HTTP suite app's wiring and connect, and each host's `listen`, listener, address and
  adoption.
- **Two paths dropped the cause.** The rocket host awaited its lift-off port and reported
  `RecvError(())` for a launch that failed, `run`'s answer discarded by `let _ =`; it now awaits the
  serving task and reports `run`'s answer. The RPC suite's `ready` reported the last call's outcome
  and nothing of a server whose `serve` had returned; `Server` now keeps how `serve` returned, and
  `ready` names it.
- **F338's case:** the server's bind error was already in the panic message, through
  `StartupError`'s `Display`, which prints the transport error's chain; batch 9's output filter cut
  it.

## Unrelated gaps found

- **F339:** `RpcCx` implements no `FromCall<Rpc>`, so `cx: RpcCx` on an RPC handler fails to
  compile, where the other three transports' handlers take their contexts. Found writing the lint
  crate.
- **F340:** a reply frame the link cannot encode is treated by the dispatcher's `send` as a
  departed caller: `debug` log, the call cancelled `Disconnected`, and the caller's own `Timeout`.
  Shown by the codec bite in decision 3.

## Needs sign-off

### S1. The reply constructors live on the contexts

Decision 2: `cx.reply`, `cx.reply_stream` on RPC and gRPC, `cx.reply_status` on gRPC, `RpcCx::codec`
public, and `ReplyValue`, `ReplyStream` and `ReplyItem` public and unsealed. The naming rule of
transports DESIGN principle 6 would name a public one-method trait after its method; the three keep
their names and `into_grpc`.

### S2. HTTP's completed body is buffered without a size bound

Decision 1's consequences: memory bounded only by the grace, and `on_stream_end` reporting the read.

### S3. The CBOR stamp and the lint crate are new CI surface

Decisions 3 and 4: one more conformance target in `cargo test --workspace`, one new workspace
member, one new CI job on `stable` clippy.

### S4. `ulo-build`'s `module_inception` allow

Decision 4, outside F337's text: generated code `ulo-build` writes, not an attribute macro's.

### S5. F339 and F340 filed, unbuilt

F339 is one `impl`. F340 needs the link's send error to tell an encoding failure from a closed lane.

### S6. DESIGN text this falsifies, left for the fold

- §2.4, the deadline paragraph, and principle 24: HTTP tells a stream "by a body with no exact size
  hint", "refusing a single payload written through `Body::stream`", becomes a body still open when
  the grace runs out, a completed one written with its exact length.
- §3.6, the cancellation bullet: "Their answer is the response when its body reports an exact size.
  A body with no exact `size_hint` ..." becomes decision 1.
- The failure-mode table's passed-deadline row: "or a body with no exact size" becomes a body still
  open at the grace's end.
- §5.1, the `fw_rpc::Reply` paragraph: an error handler builds one with `RpcCx::reply` and
  `reply_stream`, which encode with the link's codec.
- §6.1, the Reply bullet: "An error handler claiming an error with `Ok` has no public way to encode
  a message into that `Reply` ... recovering a call with a message is possible on HTTP, RPC and
  WebSocket and not here" becomes `GrpcCx::reply`, `reply_stream` and `reply_status`.
- §6.2, the deadlines bullet: "a body an error handler writes by hand carrying no mark of its
  shape" stays true; `GrpcCx::reply` and `reply_stream` produce the same erased `Reply`.
- §3.8 and §5.2, the conformance paragraphs: `report` beside each suite's trait, the scenarios
  `recovered_error` and `error_handler_answers_its_own_value`, and the CBOR stamp of TCP's suite.
- The crate table, which lists `fw-http-conformance` and `fw-rpc-conformance`: `fw-macro-lints`,
  if test crates belong there; §5.2's conformance paragraph names CI's broker job, and the
  `generated code lints` job has no paragraph to join.

## Not covered

- **The other HTTP hosts on the completed-body rule.** The change sits in `AppService`, which every
  backend and embedding shares; the new route-timeout test runs on the hyper backend.
- **A body that completes within the grace but whose data exceeds what the client accepts.**
  Nothing bounds the buffer (S2).
- **Allow-by-default lints.** The CI job runs clippy's default set with `-D warnings`. A user crate
  denying `missing_docs`, `unreachable_pub` or a pedantic group was not tried against the
  generated code.
- **A broker container's state on a failed start.** `report` gives testcontainers' error and its
  chain; the container's logs are not collected, since a failed start returns no container handle.
- **The HTTP hosts' serve loops.** Four hosts bind their own listener and run `run(..)` in a task
  whose answer is discarded, as before; a serve loop that dies after `start` shows as a refused or
  reset request in the scenario, without its cause.

## Verification

Against known violations, each change temporarily made, run, and the file restored and confirmed
byte-identical by `shasum -c`:

| Violation | Run | Failure |
| --- | --- | --- |
| `service.rs` as `e42cd467` has it (the size-hint rule) | `route_timeout` | the `Body::stream` test answered `504` where `503 "claimed"` was expected; the SSE test answered after 203.9 ms, "before the 200ms route timeout and the 500ms grace had passed"; 1 passed |
| `Buffered` reporting no exact `size_hint` | `route_timeout` | both delivered-reply tests: the body arrived chunked, `"7\r\nclaimed\r\n0\r\n\r\n"`, without `Content-Length`; 1 passed |
| `RpcCx::reply` and `encoded_stream` encoding with `Codec::Json` | TCP `conformance` and `conformance_cbor`, the new scenario | JSON stamp passed; CBOR stamp failed in `ready`, the handler path sharing `reply`: "the server did not answer within the boot budget: Err(RpcError { kind: Timeout, .. }), every server still serving" |
| `RpcCx::reply` alone encoding with `Codec::Json`, the handler path on `cx.codec()` | `conformance_cbor`, the new scenario | "the error handler's value answers the call: RpcError { kind: Timeout, message: \"the call timed out\", .. }" after 5 s (F340) |
| the conformance app's two handlers without their `#[error_handlers]` | TCP `conformance`, the new scenario | "the error handler's value answers the call: RpcError { kind: NotFound, message: \"the conformance resource does not exist\", .. }" |
| `SubstituteStream` answering its stream `.take(2)` | TCP `conformance`, the new scenario | `left: [7, 8]`, `right: [7, 8, 9]` |
| `GrpcCx::reply` and `reply_stream` answering `Status::unknown` | `replies` | both new tests: "Substituted failed: Status { code: Unknown, message: \"not encoded\", .. }"; 10 passed |
| `GrpcCx::reply`, `reply_stream` and `reply_status` answering `Status::unknown` | `deadlines` | the one-message, claimed, streamed and still-open tests; 6 passed |
| `GET /recover` without `#[error_handlers(value = Substitute)]` | hyper `conformance`, `recovered_error` | both modes: `left: (404, "{..\"detail\":\"the suite has none left\"}")`, `right: (200, "{\"name\":\"spare\"}")` |
| the reply probe's `allow` without `clippy::diverging_sub_expression` | the CI clippy command | 18 errors, "sub-expression diverges", at each `#[ulo_rpc::message]`/`event` and parameter in `src/rpc.rs` |
| `ulo-build`'s marker module without `#[allow(clippy::module_inception)]` | the CI clippy command | "module has the same name as its containing module" at `pub mod lints` in the generated `lints.v1.rs` |

The CI command, `cargo clippy -p ulo-macro-lints --all-targets --no-deps --locked -- -D warnings`,
passes on the final tree. A `--no-deps` clippy over every workspace member with `--all-targets`
reports, outside `crates/ulo/src`, only hand-written framework and test code (the `into_*`
convention in the `__private` modules, collapsible `if`s, large `Err` variants and the like), and
nothing at a macro call site.

Forced startup failures, each file restored by `shasum -c`:

- The TCP probe socket kept bound (`std::mem::forget(probe)`): `unary_round_trip` panics with "the
  conformance server did not start: transport `Rpc` failed to bind: cannot listen on
  127.0.0.1:50026: Address already in use (os error 48)". The same text appeared before this batch.
- The rocket host given a held port: before this batch, "rocket lifts off: RecvError(())"; now
  "rocket did not lift off; the app shut down on: transport `Http` failed: the rocket host server
  failed: binding failed: Address already in use (os error 48)".

Three consecutive runs per target on stable, one suite at a time, test time as libtest reports it;
the brokers with `--features integration`:

| Target | Run 1 | Run 2 | Run 3 | Result |
| --- | --- | --- | --- | --- |
| `ulo-rpc-tcp` conformance | 1.32 s | 1.31 s | 1.32 s | 22 passed |
| `ulo-rpc-tcp` conformance_cbor | 1.31 s | 1.31 s | 1.30 s | 22 passed |
| `ulo-rpc-tcp` deadline_answers | 0.21 s | 0.20 s | 0.20 s | 2 passed |
| `ulo-rpc-udp` conformance | 1.31 s | 1.30 s | 1.30 s | 21 passed, 1 ignored |
| `ulo-rpc-nats` | 3.07 s | 2.75 s | 2.69 s | 22 passed |
| `ulo-rpc-redis` | 5.11 s | 4.92 s | 5.38 s | 22 passed |
| `ulo-rpc-mqtt` | 6.22 s | 5.96 s | 5.04 s | 22 passed |
| `ulo-rpc-rabbitmq` | 29.97 s | 25.88 s | 21.40 s | 22 passed |
| `ulo-rpc-kafka` | 20.46 s | 23.01 s | 22.22 s | 22 passed |
| `ulo-http-hyper` conformance | 0.31 s | 0.31 s | 0.31 s | 36 passed, 2 ignored |
| `ulo-http-hyper` route_timeout | 0.71 s | 0.70 s | 0.70 s | 3 passed |
| `ulo-http-axum` | 0.32 s | 0.31 s | 0.31 s | 38 passed |
| `ulo-http-actix` | 1.02 s | 1.02 s | 1.02 s | 34 passed, 4 ignored |
| `ulo-http-actix` conformance_http2 | 4.05 s | 4.03 s | 4.02 s | 38 passed |
| `ulo-http-poem` | 0.73 s | 0.73 s | 0.72 s | 38 passed |
| `ulo-http-rocket` | 4.03 s | 4.01 s | 4.01 s | 38 passed |
| `ulo-http-salvo` | 0.32 s | 0.31 s | 0.31 s | 39 passed |
| `ulo-codegen-tests` replies | 0.01 s | 0.00 s | 0.00 s | 12 passed |
| `ulo-codegen-tests` deadlines | 0.51 s | 0.51 s | 0.51 s | 10 passed |

The long lines of the startup reports in the five broker suites and four HTTP hosts were wrapped
after these runs, a whitespace-only change; the HTTP hosts passed one more run each on the final
tree, and the broker suites compiled.

RabbitMQ's suite took 21 to 30 s against batch 9's 12.8 to 13.1 s. A fourth run took 28.44 s, and
two runs skipping the new scenario took 25.06 and 23.73 s, so the scenario does not account for
it. Swap stood at 12.5 to 13.4 GB of 14 GB through the broker runs, against 12.1 to 13.6 GB in batch
9; not investigated further. `postgres:18`, `redis:7`, `axllent/mailpit` and `chrislusf/seaweedfs`,
started outside this batch, ran throughout. The Docker engine answered throughout.

- `cargo check --workspace --all-targets --all-features` on stable, with
  `CFLAGS=-I/opt/homebrew/opt/openssl/include LDFLAGS=-L/opt/homebrew/opt/openssl/lib`, and
  `cargo +1.88 check --workspace --all-targets --exclude ulo-http-salvo --exclude
  ulo-graphql-async-graphql`, each print the same 17 warnings, all in `crates/ulo/src`, the set
  listed in `batch2a-cfgattr.md`. `ulo-macro-lints` declares the workspace's 1.88 and compiles on
  it.
- `cargo test --workspace --no-fail-fast`, with the OpenSSL flags: 401 passed, 0 failed, 64 ignored:
  batch 9's 362 plus the 22 CBOR scenarios, the new RPC scenario on TCP and UDP, `recovered_error`
  in both modes on six hosts, the route-timeout test and the two `replies` tests; the two new
  ignored are the `ignore` doctests on `RpcCx::reply` and `GrpcCx::reply`.
- `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --lib` over `ulo-grpc`, `ulo-http`, `ulo-rpc`,
  `ulo-build`, `ulo-rpc-macros`, `ulo-macro-lints`, `ulo-rpc-conformance` and
  `ulo-http-conformance`: no warning. With `--document-private-items`, `ulo-grpc` and `ulo-rpc` are
  clean; `ulo-http` fails on a redundant explicit link target at `service.rs:47`, a line this batch
  did not change.
