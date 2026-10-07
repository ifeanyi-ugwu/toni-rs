# Divergences: race 2b, tests batch 4, what tests batch 1 left unexercised

`race2b-tests1.md` "Unexercised" listed the WebSocket and gRPC surfaces no test had reached. This
batch drives them over real sockets: gateways through `ulo_ws::Server` and through `WsModule`'s
hand-off on `ulo_http::Server` over hyper, and gRPC services through `ulo_grpc::Server`, called by
the clients `ulo-build` generates and by tonic-health's and tonic-reflection's own clients. It
adds four compile-fail fixtures. One test exposed a bug, in the gateway attribute's closures.

Files added: `crates/ulo-ws/tests/{support/mod.rs, handoff.rs, handshake.rs, limits.rs,
messages.rs, hooks.rs}`; `crates/ulo-codegen-tests/{proto/probe.proto, proto/bare.proto,
tests/support/mod.rs, tests/replies.rs, tests/deadlines.rs, tests/enhancers.rs, tests/builtins.rs,
tests/clients.rs}`; `crates/ulo-codegen-tests/tests/ui/{meta_on_ws_message_names_another_transport,
meta_on_grpc_method_names_another_transport, grpc_request_of_another_message,
grpc_reply_of_another_message}.{rs,stderr}`. Files changed: `crates/ulo-handler-codegen/src/util.rs`,
`crates/ulo-macros/src/{enhancers/mod.rs, module_attr/providers.rs}`,
`crates/ulo-ws-macros/src/gateway.rs`, `crates/ulo-ws/Cargo.toml` (dev-dependency
`ulo-http-hyper`), `crates/ulo-codegen-tests/{Cargo.toml, build.rs, src/lib.rs,
tests/compile_fail.rs}`, `Cargo.lock`.

## The signatures

```rust
// ulo_handler_codegen::util (new; moved from `ulo-macros`, where it was `pub(crate)`)
pub fn wrap_async(closure: &syn::ExprClosure) -> syn::ExprClosure;
```

No public signature of a framework crate changed. `ulo-codegen-tests` gains the modules
`probe` (`codegen.probe.v1`) and `bare` (the package-less proto), and the dependency
`prost-types`.

## Fixes

### 1. `connect_guards(with = ..)` refused a closure `#[guards(with = ..)]` accepts

- **What forced it:** `handshake.rs`'s gateway declaring
  `connect_guards(with = |allowlist: Dep<Allowlist>| Allowed(allowlist), with(execution) = |head:
  Dep<UpgradeHead>| FromAgent(..))` failed to compile, once per closure:

  ```text
  error[E0277]: `{closure@crates/ulo-ws/tests/handshake.rs:143:16: 143:43}` is not a factory the container can call
     --> crates/ulo-ws/tests/handshake.rs:143:16
      |
  143 |         with = |allowlist: Dep<Allowlist>| Allowed(allowlist),
      |                -^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
      |                expected a closure returning a future, every parameter an injection point
  ```

  `#[guards]`, `#[interceptors]`, `#[error_handlers]` and `#[module]`'s `into` lists pass a `with`
  closure through `ulo-macros`' `wrap_async`, which puts a non-`async` body inside `async move`.
  The gateway attribute parses the same `Entry` and handed the closure to `guard_with` as written.
  DESIGN §4.1 makes `connect_guards(..)` the spelling for connect guards, which are ordinary
  `Guard<WsConnect>` entries, so one entry spelling compiled in one attribute and failed in the
  other.
- **Written:** `wrap_async` moves to `ulo_handler_codegen::util`, and `ulo-macros` imports it from
  there. The gateway attribute wraps each `with` and `with(<scope>)` closure through it, and
  `session_with` too, which had a partial wrap of its own: it kept a closure's explicit return
  type on an `async` body, which does not compile, and wrapped an `async` closure a second time.
  Both now take what `#[guards(with = ..)]` takes.

## Decisions

### 1. Where each test lives

- **WebSocket:** five files under `crates/ulo-ws/tests/`, one per surface: `handoff.rs` (the HTTP
  port, `WsModule`'s defaults, `AfterInit` from the hand-off, the drain's 1001, rooms and
  broadcast), `handshake.rs` (subprotocols, `refuse = handshake`, connect guards by value and by
  closure, `session = T`), `limits.rs`, `messages.rs` (`codec = msgpack`, messages without an `id`,
  `cancel`, enhancers and `#[meta]`) and `hooks.rs` (`OnDisconnect`'s reasons, `AfterInit` on the
  standalone server). `support/mod.rs` holds the app runner, the client and a `Record` a gateway
  writes to.
- **gRPC:** five files under `crates/ulo-codegen-tests/tests/`: `replies.rs`, `deadlines.rs`,
  `enhancers.rs`, `builtins.rs` (health and reflection) and `clients.rs` (`GrpcClientModule` and
  `outgoing`), over two new protos beside `counter.proto`.

### 2. The WebSocket client performs the handshake itself

- **Written:** `support::upgrade` writes the upgrade request, reads the response head one byte at
  a time, checks `Sec-WebSocket-Accept`, and continues with
  `WebSocketStream::from_raw_socket(.., Role::Client, ..)`. A refusal's status, headers and body
  are returned as written.
- **Why:** tungstenite's client fails a handshake whose 101 carries no `Sec-WebSocket-Protocol`
  when the request offered one (`SubProtocolError::NoSubProtocol`). RFC 6455 §4.2.2 lets the server
  echo none, and DESIGN §4.1 has the gateway decide after such a 101, which a test must see. Reading
  byte by byte keeps a Close frame written right after the 101 on the socket the client continues
  on.

### 3. The deadline tests call through a plain HTTP/2 client

- **Written:** `Running::plain_http2` builds `hyper_util`'s legacy client with `http2_only`, and
  each test builds the generated client over it with `ClockClient::with_origin`.
- **Why:** a tonic `Channel` wraps every call in `GrpcTimeout`, which enforces the request's
  `grpc-timeout` on the client and answers CANCELLED when it passes
  (`tonic-0.14.6/src/transport/channel/service/connection.rs:73`). That races the server's answer
  to the same deadline, and the answer under test is the server's.

### 4. Negative assertions are made by order, not by waiting

Every wait is bounded by `support::WAIT`, 5 s, and fails the test when it runs out. No assertion
passes because nothing arrived within a window:

- "A broadcast did not reach X": a second broadcast X is sure to receive follows it from the same
  sender. Delivery from one sender is ordered, so X's next frame being the second one rules out the
  first.
- "A message without an `id` gets no ack": the gateway declares `max_inflight = 1`, and a message
  with an `id` follows. Its answer arriving first rules out an ack for the first.
- "`max_inflight = 1` reads nothing more": the held message's answer must precede the second
  message's, and the gate's log must read `hold started`, `opened by the test`, `hold done`,
  `opened by a message`. A round trip on a second connection before the test opens the gate gives a
  connection that kept reading the time to run the second message first.
- "A refused connection never reaches `on_disconnect`": read after `app.stop()`, whose drain waits
  for every connection's task.
- "A client answering each Ping outlives `pong_timeout`": three Pings, 300 ms, then a message
  answered on the same connection.

### 5. The outbound-queue tests rely on the current-thread runtime

- **Written:** the handler sends its frames through `Connection::send` in one poll, on the
  runtime `#[tokio::test]` builds, so the connection's loop runs only after the last send. The
  module doc says so.
- **Why:** `max_outbound` and `overflow` are decided by how many frames are queued when the loop
  next takes them, and nothing else orders the handler's pushes against the loop. `drop_oldest`
  sends its burst without an `id`, since an answer's `complete` would take the second place and
  leave "drop the oldest" indistinguishable from "drop everything queued" (decision 7).

### 6. The well-known types exercise both of `ulo-build`'s marker paths

`google.protobuf.StringValue` reaches the marker as `::prost::alloc::string::String`, the Rust
type prost maps a wrapper to, and `Empty` as `()`. Neither is a `::prost_types` path, so
`Known.Since` takes a `google.protobuf.Timestamp` and replies a `google.protobuf.Duration`, which
the markers name as `::prost_types::Timestamp` and `::prost_types::Duration`.

### 7. Two tests rewritten after a mutation passed them

- `to_all_reaches_every_gateway_and_a_room_broadcast_only_its_members` broadcast to the room through
  `rooms.namespace("lobby")`, which excludes the annex gateway's connection by gateway before
  membership is read. With room membership ignored (`Audience::Room(_) => true`) it still passed.
  The lobby gained a `room` handler broadcasting with `rooms.to_room("lobby")`, unscoped, so only
  membership keeps the annex's connection out.
- `drop_oldest_keeps_the_newest_messages_and_the_connection` sent its burst with an `id`. With
  `DropOldest` clearing the queue instead of popping its front, it still passed (decision 5). It now
  sends the burst without one and expects `"4"` then `"5"`.

### 8. A guard's refusal reaches a method's error handlers

`enhancers.rs`'s first run failed: `Fragile` called without a token answered FAILED_PRECONDITION,
not PERMISSION_DENIED. Its error handler answered every error with FAILED_PRECONDITION, and the
guard's `GuardRejected` is offered to the error handlers like any error (DESIGN §2.4). The test was
wrong. Its error handler now claims the handler's own `Refusal` alone.

## Observed, not changed

- **An error handler claiming a gRPC error with `Ok` has no public way to encode a message.**
  `Reply` is `http::Response<tonic::body::Body>`, and `encode_one` is crate-private. The deadline
  test's claim answers `Status::unavailable(..).into_http()`.
- **A caller through a tonic `Channel` never sees the reply the U2 grace produces.** Its own
  `GrpcTimeout` answers CANCELLED at the same deadline (decision 3). The grace's answer reaches a
  caller whose client enforces no deadline of its own, and the error handlers run, and log or
  count, either way.
- **Attribute macros see an impl's macro calls unexpanded.** `#[message]` handlers written by a
  `macro_rules!` call inside a `#[routes]` impl are not seen by `#[ulo_ws::gateway]`, which refuses
  the impl as a gateway with no handler. `#[routes]` reads an impl the same way.
- **Under `refuse = close`, `OnConnect` runs after the 101.** State it writes, a room joined or a
  directory entry, is visible to other connections only once the connect phase has finished. The
  rooms tests wait for one round trip on each connection, since no message is read before that.

## Unexercised

- **WebSocket:** `cfg_attr` around `#[message]`; the `ViaBoxError` arm (an error that boxes but is
  no `Into<CallError>`); a hand-written `Gateway`; a failing `session_with` refusing with 1011;
  `with(singleton)` over a per-execution read refused at `wire()`; `ping_interval = Unbounded`;
  `wss`; the hand-off on an embedding host declaring `upgrades`, and on the adapters other than
  hyper; `ulo-ws-redis`'s adapter.
- **gRPC:** the `ValueViaBoxError` arm with an error that is not a `Status`; `with_grpc_code`;
  `Details` in `grpc-status-details-bin`; `build_client(false)` and `out_dir`; the refusal of a
  generic handler method; `ClientTls`; `max_inflight`, `max_per_connection` and
  `max_concurrent_streams`; a `grpc-encoding` other than `identity`; the 4 MiB message limit;
  UNAVAILABLE during the drain; `ClientCancelled` on an abandoned call; a deadline that passes
  before routing, offered to the global error handlers alone.

## Verification

- Every new target passes three consecutive runs on stable (rustc 1.98.1), `cargo test -p <crate>
  --test <target>`, test time as libtest reports it:

| Target | Tests | Run 1 | Run 2 | Run 3 |
| --- | --- | --- | --- | --- |
| `ulo-ws` `handoff` | 9 | 0.01 s | 0.02 s | 0.00 s |
| `ulo-ws` `handshake` | 10 | 0.01 s | 0.02 s | 0.00 s |
| `ulo-ws` `limits` | 9 | 0.32 s | 0.33 s | 0.31 s |
| `ulo-ws` `messages` | 9 | 0.02 s | 0.02 s | 0.00 s |
| `ulo-ws` `hooks` | 8 | 0.01 s | 0.02 s | 0.00 s |
| `ulo-codegen-tests` `replies` | 10 | 0.01 s | 0.02 s | 0.00 s |
| `ulo-codegen-tests` `deadlines` | 7 | 0.52 s | 0.53 s | 0.51 s |
| `ulo-codegen-tests` `enhancers` | 7 | 0.01 s | 0.02 s | 0.00 s |
| `ulo-codegen-tests` `builtins` | 6 | 0.01 s | 0.00 s | 0.00 s |
| `ulo-codegen-tests` `clients` | 5 | 0.01 s | 0.00 s | 0.00 s |
| `ulo-codegen-tests` `compile_fail` | 1 (8 fixtures) | 0.86 s | 0.45 s | 0.44 s |

  `limits` spends its time on the keep-alive Pings, 100 ms apart, and `deadlines` on the 200 ms
  deadline and the 300 ms grace it measures.

- Against known violations, each change applied to the source, the named test run, the file
  restored and compared by SHA-256. Each of these failed its test:
  - WebSocket: a `port = own` gateway served on the HTTP port; `except` ignored; `to_client`
    reaching everyone; room membership ignored; `leave` a no-op; `WsModule`'s defaults ignored by
    the hand-off; the hand-off's `AfterInit` not run; the subprotocol chosen by the client's order;
    only the first `Sec-WebSocket-Protocol` line read; `refuse = handshake` ignored; a 401 without
    `WWW-Authenticate`; a slot taken before a handshake refusal; `max_inflight` ignored; the Pong
    deadline ignored; a Pong not clearing it; an ack for a message without an `id`; `cancel`
    ignored; `on_disconnect` run for a refused connection; the drain reported as `ServerClose`;
    `DropOldest` clearing the queue; overflow never closing; `session = T` building no session; a
    connect guard's refusal admitting the connection; MessagePack frames refused as malformed; a UTF-8 error
    reported as `Lost`; a client's close reason dropped.
  - gRPC: a `Status` rendered by its kind; `Response<S>` dropping its metadata; `GrpcMetadata`
    extracted empty; the error handlers given no grace; the pipeline dropped before the cancel; the
    grace bounding nothing; a nine-digit `grpc-timeout` accepted; the reply body ignoring the
    deadline; health left SERVING in the drain; known services never set SERVING; `GrpcHealth`'s
    reporter ignored; no `v1alpha`; reflection served under `reflection(false)`; 401 translated as
    UNKNOWN; a scoped entry covering another service; `outgoing` forwarding nothing.
- Each compile-fail fixture fails for its mismatch alone: with the mismatch removed from all four
  new fixtures (`#[meta]` deleted, `Message<AddRequest>` taken, `Sum` returned), trybuild reports
  each "Expected test case to fail to compile, but it succeeded." The fixtures were restored and
  compared byte for byte with a copy taken before the edit. The existing four snapshots are
  unchanged.
- `cargo check --workspace --all-targets` on stable and `cargo +1.88 check --workspace
  --all-targets --exclude ulo-http-salvo --exclude ulo-graphql-async-graphql` print the same 17
  warnings, all in `crates/ulo/src`, and none in a file this batch touches.
- `cargo test --workspace --no-fail-fast`: 350 passed, 0 failed, 61 ignored, 398 s wall time.
