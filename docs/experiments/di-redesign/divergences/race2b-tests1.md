# Divergences: race 2b, tests batch 1, the first expansion of the WebSocket and gRPC attributes

`transports/DIVERGENCES_2B.md` §8 lists the generated code no compile had reached:
`#[ulo_ws::gateway]`, `#[ulo_ws::message]`, `#[ulo_grpc::method]`, `ulo_build`'s output, X24's
mismatch arm and the gRPC shape check. This batch expands each of them in a test and runs what
compiles over a real socket. Every expansion compiled on its first build. The one source change is
a diagnostic's span.

Files added: `crates/ulo-ws/tests/attributes.rs`; `crates/ulo-codegen-tests/{Cargo.toml, build.rs,
proto/counter.proto, src/lib.rs, tests/grpc.rs, tests/compile_fail.rs, tests/ui/*.rs,
tests/ui/*.stderr}`. Files changed: `Cargo.toml` (member `crates/ulo-codegen-tests`, workspace
dependency `trybuild`), `Cargo.lock`, `crates/ulo-ws/Cargo.toml` (dev-dependencies),
`crates/ulo-grpc-macros/src/method.rs`.

## The signatures

No public signature changed.

## Fixes

### 1. The reply check's span is the return type, not the `->`

- **What forced it:** the first `.stderr` snapshot of `grpc_unary_reply_for_a_streaming_method.rs`
  put the caret on one character, the `-` of `->`:

  ```text
  error[E0599]: no method named `checked` found for struct `ulo_grpc::__private::Checked<(ulo_grpc::__private::Shaped<false>, Number), (ulo_grpc::__private::Shaped<true>, Number)>` in the current scope
    --> tests/ui/grpc_unary_reply_for_a_streaming_method.rs:12:55
     |
  12 |     async fn count(&self, req: Message<CountRequest>) -> Number {
     |                                                       ^ method not found in ..
  ```

  The probe was spanned at `item.sig.output.span()`. On stable, a span over several tokens keeps
  only the first one's, and `ReturnType::Type`'s first token is the `->`.
- **Written:** the probe is spanned at the return type, `ReturnType::Type(_, ty) => ty.span()`,
  and at the method's name for a handler with no return type. The caret now covers `Number`
  (`12:58`, `^^^^^^`). An `impl Stream<..>` return gets the `impl` token, for the same stable
  reason.

No expansion needed a fix to compile. The §8 uncertainties, each now exercised by compiling and
running:

| §8 uncertainty | Where it is exercised | Result |
| --- | --- | --- |
| W's `Answer<M>` inference at a `#[message]` call | a handler per reply kind: `()`, a `Serialize` value, `Frame`, `Reply`, a stream of `Result<u32, E>`, and `Result<_, E>` around each | infers, each answer on the wire as §4.2's envelope states |
| the `session_with` wrapping | `session_with = \|head: Dep<UpgradeHead>\| Visitor { .. }`, a closure with a non-`async` body | wrapped in `async move`, one session per connection |
| G's six-arm probe with the const-generic turbofish | a handler per shape: `Result<Response<Sum>, E>`, `impl Stream<Item = Result<Number, E>>` (two), `Result<Sum, E>`, a bare `Number` | each picks its arm |
| the anonymous const inside a generic impl | `#[routes] impl<F: Factor> Scaled<F>` with a `#[method]` handler, and `impl<W: Word> Echo<W>` under `#[ulo_ws::gateway]` | compiles and serves |
| X24's E0308 arm | `#[meta(Timeout::after(..))]` on an RPC handler, `#[meta(BodyLimit(1024))]` on an RPC `#[routes]` impl | E0308 at the value, ``expected `()`, found `MetaMismatch<Rpc, Http>` `` |
| `ulo_build`'s markers, the `GrpcClient` impl and `FILE_DESCRIPTOR_SET` | `build.rs` over a proto with two services; the client built by `GrpcClient::from_channel`; the set passed to `Server::file_descriptor_set` | compile; the markers route all six calls |

## Decisions

### 1. Where each test lives

- **WebSocket:** `crates/ulo-ws/tests/attributes.rs`. The test needs only `ulo-ws`'s public surface
  and a client, with no build step, so `cargo test -p ulo-ws` runs it beside the crate it checks.
  Its dev-dependencies are `ulo-tokio`, tokio's `rt-multi-thread` and `time`, and
  tokio-tungstenite's `connect`.
- **gRPC and the compile-fail cases:** one `publish = false` member, `crates/ulo-codegen-tests`.
  The build step needs a package with a `build.rs`. A trybuild fixture has no `OUT_DIR` of its own,
  and it reaches the generated markers through the member's library, `ulo_codegen_tests::pb`. The
  X24 fixtures need `ulo-http` and `ulo-rpc` together, which no transport crate depends on.

### 2. The snapshots run on rustc 1.98.1 only

- **Written:** the member's `build.rs` reads the `release:` line of `$RUSTC -vV` and emits
  `cargo::rustc-cfg=ulo_snapshot_rustc` when it is `1.98.1`, declared with
  `cargo::rustc-check-cfg`. The test carries
  `#[cfg_attr(not(ulo_snapshot_rustc), ignore = "the .stderr snapshots are rustc 1.98.1's")]`.
  The regeneration command is in `tests/compile_fail.rs`'s header.
- **Why:** all four snapshots are rustc's own E0599 and E0308 text, which rustc rewords between
  releases. An ignored test still shows in the output with its reason. A runtime check of
  the toolchain inside the test would report a pass where nothing was checked.

### 3. One fixture per case

Four fixtures: X24 at the method tier and at the impl tier, and the gRPC check on each side, a
unary reply for the server-streaming `Count` (`Checked`, the reply probe) and a `Streaming<_>`
parameter for the unary `Add` (`ParamCheck`, the parameter probe). The two gRPC sides go through
different probes. X24's two tiers are written from separate lists, `MetaTokens::controller` and
`MetaTokens::method`, and a fault in either reaches only its own fixture.

### 4. A connect guard named by type is bound in the module

`connect_guards(TokenGuard)` is a DI key, like every by-type enhancer entry: the first run failed
`wire()` with ``missing dependency `TokenGuard` ``, ``needed by Chat::connect (enhancer `TokenGuard`)
in Root``. The test makes `TokenGuard` `#[injectable]` and calls `m.provide::<TokenGuard>()`. This
is the design's rule, unchanged.

### 5. The descriptor set goes to `OUT_DIR`

`file_descriptor_set_path(out_dir.join("codegen.bin"))`. A relative path resolves against the
package root, which would write a build artifact into the source tree.

## Observed, not changed

- **A client that does not answer the drain's 1001 holds shutdown for the drain timeout.** The first
  WebSocket run took 10.01 s per test, every test the same, with the client socket still open at
  `AppHandle::close`. Dropping the client first: 0.00 s (Probed). The read loop sends 1001 and then
  waits for the client's Close under `close_deadline`, which is `pong_timeout`, 30 s by default
  (Read: `ulo-ws/src/connection.rs:832-842`). The app's drain timeout, 10 s by default
  (`ulo/src/app/mod.rs:73`), ends it first. That
  matches §4's drain as written. The test hangs up each client before closing the app.

## Unexercised

- **WebSocket:** a gateway on the HTTP server's port through `WsModule`'s hand-off (both gateways
  here are `port = own`); `codec = msgpack`; `session = T`; connect guards by value, closure or
  scoped closure; `refuse = handshake`; `subprotocols`; the limit settings, including the
  integer-literal rewrite to `Count::Max`; `OnDisconnect` and `AfterInit` found by the hook probe
  (only `OnConnect` is implemented); method-tier enhancers and `#[meta]` on a `#[message]` handler;
  `cfg_attr` around `#[message]`; the `ViaBoxError` arm (an error that boxes but is no
  `Into<CallError>`); a message without an `id`; `cancel`; rooms and broadcast.
- **gRPC:** `Response<S>` around a stream; the three stream arms under a `Result`; the
  `ValueViaBoxError` arm; a `tonic::Status` returned as the error; `GrpcMetadata` as a parameter;
  `GrpcClientModule`; reflection and health served and called; enhancers, `PreDispatch`, deadlines;
  `build_client(false)` and `out_dir`; a proto using `google.protobuf.Empty` or another well-known
  type; a proto without a `package`; the refusal of a generic handler method.
- **Compile-fail:** X24 on a WebSocket or gRPC handler (both fixtures put HTTP metadata on RPC); a
  gRPC message-type mismatch with the shape right.

## Verification

- `cargo test -p ulo-ws --test attributes`: 11 passed in 0.01 s. `cargo test -p ulo-codegen-tests`:
  `grpc` 6 passed, `compile_fail` 1 passed, on rustc 1.98.1.
- The hook probe's implemented arm is what the `on_connect` test reads: with `impl OnConnect` moved
  to an unrelated type, the banned visitor is admitted and the test fails, "the refused connection
  was not closed within 5s".
- Each compile-fail case catches its mismatch: with the mismatch removed from all four fixtures
  (the `#[meta]` attribute deleted, `Count` answering a stream, `Add` taking `Message<AddRequest>`),
  trybuild reports each "Expected test case to fail to compile, but it succeeded." The fixtures were
  restored and compared byte for byte with a copy taken before the edit.
- The gate: `cargo +1.88 test -p ulo-codegen-tests --test compile_fail` reports
  `0 passed; 0 failed; 1 ignored`; on 1.98.1, `1 passed`.
- `cargo check --workspace --all-targets` on stable and
  `cargo +1.88 check --workspace --all-targets --exclude ulo-http-salvo --exclude ulo-graphql-async-graphql`
  pass. The only warnings are the 17 in `crates/ulo/src` that the first build of this batch already
  printed, before any source change.
- `cargo test --workspace` passes.
- `.github/workflows/ci.yml` on this branch is `master`'s and names neither new test. Nothing in CI
  runs them.
