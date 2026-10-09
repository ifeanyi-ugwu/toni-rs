# Divergences: runtime neutrality, stage b: `Upgraded` on `futures-io` with tokio's traits behind `tokio-io`, `Embed::take_upgrade`, and `ulo-net`'s TLS ending at rustls's `ServerConfig`

The thirty-fourth response splits runtime neutrality into six stages; this is the second. `ulo-http`
loses every normal-dependency path to tokio: `Upgraded` is built from and implements `futures-io`'s
`AsyncRead` and `AsyncWrite`, and the `tokio-io` feature adds a constructor from tokio's traits and
impls of them, both through `tokio-util`'s `compat` layer. The tower embedding's hyper upgrade moves
to the axum adapter through a defaulted `Embed::take_upgrade`, which takes `hyper` and `hyper-util`
out of `ulo-http`. `ulo-net` drops `tokio-rustls`: `Tls::load` answers an `Arc<rustls::ServerConfig>`,
`Backend::bind` and `ulo_hyper_serve::Serve::new` take it, and the two tokio crates that accept
connections, `ulo-hyper-serve` and `ulo-rpc-tcp`, build `tokio_rustls::TlsAcceptor` from it. Every
tokio-based caller enables `tokio-io`. `ulo-rpc`, `ulo-ws`'s engine and the smol work are unchanged.

Files changed: the workspace `Cargo.toml` and `Cargo.lock`; `crates/ulo-http/{Cargo.toml,
src/request.rs, src/embed.rs, src/backend.rs, src/server.rs, tests/common/mod.rs, tests/upgraded.rs
(new)}`; `crates/ulo-net/{Cargo.toml, src/lib.rs, src/tls.rs, tests/tls.rs (new),
tests/fixtures/{ca.pem, localhost.pem, localhost-key.pem} (new)}`;
`crates/ulo-hyper-serve/src/{lib.rs, serve.rs}`; `crates/ulo-http-hyper/{Cargo.toml,
src/backend.rs, src/convert.rs, tests/tls.rs (new)}`; `crates/ulo-http-axum/{Cargo.toml,
src/lib.rs}`; `crates/ulo-http-{salvo,poem,rocket}/Cargo.toml` with `src/handler.rs`,
`src/endpoint.rs` and `src/upgrade.rs`; `crates/ulo-http-conformance/Cargo.toml`;
`crates/ulo-ws/{Cargo.toml, src/server.rs}`; `crates/ulo-grpc/src/server.rs`;
`crates/ulo-rpc-tcp/{src/link.rs, tests/conformance_tls.rs (new)}`; F361 and F362 filed in the
workspace's `FRAMEWORK_GAPS.md`. The tree is `2eab4b21` plus this stage.

## The signatures

```rust
// ulo_http (crates/ulo-http/src/request.rs)
pub struct Upgraded { /* private: Pin<Box<dyn futures_io::AsyncRead + AsyncWrite + Send>> */ }
impl Upgraded {
    pub fn new(io: impl futures_io::AsyncRead + futures_io::AsyncWrite + Send + Unpin + 'static) -> Self;
    #[cfg(feature = "tokio-io")]
    pub fn from_tokio(io: impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin + 'static) -> Self;
}
impl futures_io::AsyncRead for Upgraded {}
impl futures_io::AsyncWrite for Upgraded {}
#[cfg(feature = "tokio-io")] impl tokio::io::AsyncRead for Upgraded {}
#[cfg(feature = "tokio-io")] impl tokio::io::AsyncWrite for Upgraded {}

// ulo_http::embed::Embed, a new defaulted associated function
fn take_upgrade(extensions: &mut http::Extensions) -> Option<OnUpgrade> { None }

// ulo_http::Backend
fn bind(
    &mut self,
    listeners: Vec<BoundListener>,
    tls: Option<Arc<ulo_net::rustls::ServerConfig>>,   // was Option<ulo_net::TlsAcceptor>
    svc: AppService,
    cfg: &HttpConfig,
) -> impl Future<Output = Result<(), BoxError>> + Send;

// ulo_net
impl Tls {
    pub fn load(&self, alpn: &[&[u8]]) -> Result<Arc<rustls::ServerConfig>, TlsError>;   // was Result<TlsAcceptor, TlsError>
}
pub use rustls;                       // new
// removed: pub use tokio_rustls::TlsAcceptor;

// ulo_hyper_serve
impl Serve {
    pub fn new(listeners: Vec<BoundListener>, tls: Option<Arc<ServerConfig>>, config: &ServeConfig) -> io::Result<Serve>;   // was Option<TlsAcceptor>
}
```

```toml
# ulo-http
[features]
tokio-io = ["dep:tokio", "dep:tokio-util"]
```

`OnUpgrade`, `Request`, `ConnInfo` and `TlsInfo` are unchanged. `Axum` overrides
`take_upgrade`; no other `Embed` does. Private: the `tls` field of `ulo-http`'s `Prepared`, of
`ulo-grpc`'s and of `ulo-ws`'s, each now `Option<Arc<ServerConfig>>`. No handler, module or
controller signature changes. The workspace gains `futures-io = "0.3"` and `tokio-util = { version
= "0.7", default-features = false }`; `ulo-http` loses `hyper`, `hyper-util` and its normal `tokio`
and gains `futures-io`, with `tokio` and `tokio-util` (`compat`) optional; `ulo-net` loses
`tokio-rustls`; `ulo-http-axum` gains `hyper` and `hyper-util` (`tokio`).

## Decisions

### 1. `Upgraded` holds `futures-io` and reaches tokio through `compat` in both directions

- **Storage:** the boxed I/O is a `futures-io` object, and the `futures-io` impls forward to it, as
  the tokio impls did before.
- **`from_tokio`:** wraps the I/O in `tokio_util::compat::Compat` (`TokioAsyncReadCompatExt::compat`),
  which implements both `futures-io` traits for a tokio reader and writer, and passes it to `new`.
- **The tokio impls:** each call wraps the boxed I/O, `Pin<&mut dyn Io>`, in a fresh `Compat` and
  calls tokio's trait on it. `Compat`'s read and write impls keep no state between calls; its only
  field beside the inner value is a seek position the seek impls use
  (`tokio-util-0.7.19/src/compat.rs`, lines 218-330). Both directions of the conversion are
  `tokio-util`'s code, and `ulo-http` writes none of it.
- **The cost:** an `Upgraded` built from tokio I/O and read through tokio's traits crosses two
  `Compat`s. The tokio-reads-futures direction initializes the unfilled part of the caller's
  `ReadBuf` before each read, as `compat.rs:229` does, since a `futures-io` reader receives a
  `&mut [u8]`. The response's answer 3 places performance at this layer outside the concern; the
  alternatives are under S3.
- **No vectored writes:** neither the old impls nor the new override `poll_write_vectored`.

### 2. The feature is `tokio-io`, off by default

- **What it adds:** `Upgraded::from_tokio` and the two tokio impls, with optional `tokio` (no
  features: the traits live in `tokio::io` unconditionally) and `tokio-util` with `compat`.
- **Who enables it:** every crate that builds an `Upgraded` from tokio I/O or reads one through
  tokio's traits: `ulo-http-hyper`, `ulo-http-axum`, `ulo-http-salvo`, `ulo-http-poem`,
  `ulo-http-rocket`, `ulo-http-conformance` (its echo handler) and `ulo-ws` (its gateway). Each
  manifest line names why; `ulo-ws`'s says stage d removes it.
- **Who does not:** `ulo-http-actix` (no upgrades), `ulo-graphql-http`, `ulo-graphql-ws`,
  `ulo-grpc`, `ulo-hyper-serve`. `ulo-graphql-ws` reads no `Upgraded`; it reaches one through
  `ulo-ws`.
- **Compat over enabling it per caller:** a caller could convert with `compat` itself; the feature
  keeps one conversion in `ulo-http` and no `tokio-util` dependency in five adapters.

### 3. hyper leaves `ulo-http`: `Embed::take_upgrade`

- **The finding:** hyper 1 depends on tokio unconditionally (`tokio` with `sync`), so the
  `hyper` dependency alone put tokio in `ulo-http`'s tree, whatever `hyper-util`'s features. The
  one use was `embed::Service`'s tower path taking `hyper::upgrade::OnUpgrade` out of the request's
  extensions (`embed.rs`, the old `convert`).
- **The move:** `Embed` gains `take_upgrade(&mut http::Extensions) -> Option<OnUpgrade>`, defaulted
  to `None`. The tower path calls `A::take_upgrade` on every request and keeps the answer only
  where `A::limits().upgrades` holds, as it kept hyper's future before. `Axum` overrides it with the
  code that was in `ulo-http`, so axum's adapter now depends on `hyper` and `hyper-util` as salvo's
  already did.
- **Why the trait and not a feature:** only axum uses the tower path, and every other hyper host
  (salvo, poem, rocket, the hyper backend) already converts hyper's future in its own crate. A
  `hyper` feature on `ulo-http` would keep a host's type in the transport crate. See S1.
- **What catches a host that forgets the override:** the conformance scenario `upgrade_echoes_a_frame`,
  which every host declaring `upgrades` runs; with axum's override answering `None` it fails in both
  modes (Tests).

### 4. Every other tokio use in `ulo-http`

| Use | Kind | Outcome |
| --- | --- | --- |
| `tokio = { features = ["io-util"] }`, the `Upgraded` traits | normal | moved behind `tokio-io`, without `io-util`, which `ulo-http` never used |
| `hyper`, for `hyper::upgrade::OnUpgrade` in `embed.rs` | normal | removed; the conversion is axum's (decision 3) |
| `hyper-util` with `tokio`, for `TokioIo` in `embed.rs` | normal | removed with it |
| `ulo-net`, through `tokio-rustls` | normal | removed (decision 5) |
| `tokio` (`macros`, `rt`, `time`, `sync`, now `io-util`) and `ulo-tokio` | dev | kept: the tests' runtime and timer; `io-util` for `tests/upgraded.rs` |

`ulo-http` spawns nothing; the two mentions of a spawn in its source are docs saying it does not
(`embed.rs:26`, `:845`) and that an upgrade handler spawns its own task (`upgrade.rs:43`). Nothing
needed `AppHandle::runtime()`.

### 5. TLS: option (b), `ulo-net` ends at rustls's `ServerConfig`

- **Who uses `ulo-net`'s TLS and what for:** `ulo-http`'s `Server` (`load` with `h2`, `http/1.1`,
  handed to the backend), `ulo-grpc` (`h2`) and `ulo-ws`'s standalone server (`http/1.1`), all three
  into `ulo_hyper_serve::Serve::new`, which wraps each accepted `tokio::net::TcpStream` and reads
  ALPN and SNI off the session (`handshake.rs`); `ulo-rpc-tcp` (`load(&[])`) wraps its own
  accepted streams. `ulo-rpc-udp` uses `ulo-net` for endpoints alone. Every consumer needs the
  same two things: a configuration with its ALPN set, and a handshake over its own runtime's stream.
- **Why (b):** the configuration is runtime-free and the handshake is not. `tokio-rustls` and
  `futures-rustls` both build their acceptor with `TlsAcceptor::from(Arc<ServerConfig>)`, so a
  configuration is the one value both can start from. A smol backend or link adds `futures-rustls`
  and writes that one line; certificate loading, the PEM error that names the file, the key-match
  check, the explicit `ring` provider and ALPN stay in `Tls::load`, once.
- **Why not (a):** `futures-rustls` in `ulo-net` gives every tokio consumer a `futures-io` stream
  to convert back: `ulo-hyper-serve` would wrap each connection in a `Compat` and then `TokioIo`,
  paying the buffer initialization of decision 1 on every HTTP and gRPC read, not only on
  upgraded connections, to serve a runtime the workspace does not yet run.
- **The re-export:** `pub use rustls;`, as `tokio-rustls` and `futures-rustls` both do, so a
  backend names `ulo_net::rustls::ServerConfig` and cannot reach a second rustls version.
  `TlsError::Rustls(rustls::Error)` already named the crate.
- **Where the acceptor is built:** `Serve::new` builds `tokio_rustls::TlsAcceptor` from the
  configuration, so `ulo-http-hyper`, `ulo-grpc` and `ulo-ws` name no TLS crate; `ulo-rpc-tcp`
  builds its own in `prepare`, where it built none before. `tokio-rustls` now sits in exactly those
  two tokio crates.
- **Unchanged:** `FD_CLOEXEC` on an inherited socket, take-once, and the listen-state checks
  (`activation.rs`, `listener.rs`, `endpoint.rs`), none of which this stage touched.

### 6. The tests' certificate

- **The fixtures:** `crates/ulo-net/tests/fixtures/` holds a P-256 test CA (`ca.pem`) and a
  leaf it signed for `DNS:localhost` and `IP:127.0.0.1` (`localhost.pem`, PKCS#8 key
  `localhost-key.pem`), both valid until 2126, generated with OpenSSL 3.6. The CA's key is not
  kept. A leaf distinct from its anchor is needed because webpki refuses a CA certificate used as
  an end entity.
- **Sharing:** `ulo-http-hyper`'s and `ulo-rpc-tcp`'s tests read them with
  `include_bytes!("../../ulo-net/tests/fixtures/..")`, a path inside the workspace that no
  published library target compiles. See S6.

## The tests

- **`crates/ulo-http/tests/upgraded.rs`, three tests,** each bounded by a five-second timeout:
  - `a_futures_io_connection_reads_back_what_was_written`, built by `new` over a loopback
    `futures-io` type and driven through `futures-io`'s extension traits; runs with or without the
    feature.
  - `tokio_io::tokio_traits_reach_a_futures_io_connection`, the same loopback driven through tokio's.
  - `tokio_io::a_tokio_connection_carries_bytes_through_either_trait`, `from_tokio` over one end
    of `tokio::io::duplex`: a write and a read through each trait family against the peer end, then
    `shutdown` read by the peer as the end of the stream.
  - The `tokio_io` module is `#[cfg(feature = "tokio-io")]`: `cargo test -p ulo-http` runs one test,
    with `--features tokio-io` three, and `cargo test --workspace` three, the feature being unified
    on by its tokio-based members.
- **`crates/ulo-net/tests/tls.rs`, three tests:**
  `a_loaded_configuration_completes_a_handshake_without_a_runtime` (a rustls `ServerConnection`
  over `Tls::load`'s answer and a `ClientConnection` trusting the test CA exchange records through
  `Vec<u8>` buffers until both finish handshaking, ALPN settles `h2`, and a record carries `ping`),
  `load_offers_the_protocols_it_is_given_in_order`, and
  `a_key_that_is_not_the_certificates_is_refused` (`TlsError::Rustls`).
- **`crates/ulo-http-hyper/tests/tls.rs`, two tests,** the conformance suite's app on
  `ulo_http_hyper::Server` with `.tls(..)` at port 0, a `tokio-rustls` client trusting the test CA:
  `alpn_settles_the_protocol_the_client_offers` (`h2` when offered first, `http/1.1` when offered
  alone) and `an_upgrade_over_tls_carries_bytes_both_ways` (a 101 from the suite's `/echo` handler,
  then two frames echoed over the TLS connection), which runs the hyper upgrade through
  `TokioIo`, `from_tokio` and the echo handler's tokio reads.
- **`crates/ulo-rpc-tcp/tests/conformance_tls.rs`,** the RPC conformance suite, 25 scenarios, with
  the server link on `.tls(Tls::from_pem(..))`. The client link speaks no TLS, so it connects to
  the suite's `Relay`, which forwards to a bridge in the test file that carries each connection on
  over `tokio-rustls`. `disrupt` cuts the relay, and `client_connections` counts its connections.
- **Coverage before this stage:** no test on the branch served TLS; the existing
  `upgrade_echoes_a_frame` scenario covered the upgraded connection on every host declaring
  `upgrades`. F361 records what still has no TLS test.

### Before and after

Each break rewrote one span through a script that asserted the span occurred once, ran the named
tests, and wrote the file back byte for byte, checked by hash; the full output of every run is in
the scratchpad (`runtime-b/broken/`). Every run reported its tests by name.

| Break | Tests run | Result |
| --- | --- | --- |
| `Upgraded`'s `futures-io` `poll_read` answers `Ok(0)` | `tests/upgraded.rs` with `tokio-io`; hyper `tests/tls.rs` | 2 of 3 failed, `read_exact` at EOF; the tokio-trait test and both TLS tests passed, the tokio impls reading the boxed I/O and not `Upgraded`'s `futures-io` impl |
| tokio `poll_read` on `Upgraded` answers `Ok(())` with nothing read | same | the two `tokio_io` tests failed; `an_upgrade_over_tls_carries_bytes_both_ways` failed, the echo handler reading EOF and closing without `close_notify` |
| tokio `poll_write` on `Upgraded` answers the length and writes nothing | same | the two `tokio_io` tests failed (early EOF; the five-second bound); the TLS upgrade test failed, "`ping` did not arrive in time" |
| `Serve::new` drops the configuration | hyper `tests/tls.rs` | both failed at the handshake: `InvalidContentType`, the server answering in plaintext |
| `Tls::load` ignores `alpn` | `ulo-net`'s `tests/tls.rs`; hyper `tests/tls.rs` | the two ALPN assertions in `ulo-net` failed, and `alpn_settles_the_protocol_the_client_offers`; the upgrade test passed, a client offering only `http/1.1` needing no ALPN |
| `ulo-rpc-tcp` serves without the handshake | `unary_round_trip`, `client_close`, `recovery_after_disrupt` over TLS | 3 failed; the bridge reported each failed handshake |
| axum's `take_upgrade` answers `None` | `ulo-http-axum`'s conformance, filtered to `echo` | `nested::upgrade_echoes_a_frame` and `fallback::upgrade_echoes_a_frame` failed: "the host declares `upgrades` and did not switch protocols" |

The first run of the `poll_write` break hung: `tests/upgraded.rs` then had no timeout, and the
duplex peer's `read_exact` waited forever. The test binary was killed, the script restored the file
by hash, the three tests gained the five-second bound, and the break ran again with the result in
the table.

## Left for the transports DESIGN fold

- §1's crate table: `fw-net`'s row (line 26) gains "TLS loading with rustls, ending at its
  `ServerConfig`; no runtime"; `fw-hyper-serve`'s row (line 27) gains "the TLS handshake through
  `tokio-rustls`".
- §2.7's TLS paragraph (line 343): `Tls::load` answers `Arc<rustls::ServerConfig>`, which a
  runtime's TLS crate wraps (`tokio-rustls` in `fw-hyper-serve` and the TCP link,
  `futures-rustls` for a backend over `futures-io`); `fw_net::rustls` re-exported.
- §3.5, line 535: `Request::upgrade`'s comment, "an AsyncRead + AsyncWrite + Send + Unpin stream",
  names `futures-io`'s traits, and tokio's under `tokio-io`.
- §3.7: the `bind` signature (line 517) takes `Option<Arc<ServerConfig>>`; the settings paragraph
  (line 506) drops `TlsAcceptor` from the types a backend alone names, `ServerConfig` taking its
  place; line 541's `Serve::new(listeners, tls, &ServeConfig)` takes the configuration, and its
  "`fw-http` does not depend on it" now holds for tokio, hyper and `tokio-rustls` alike.
- §3.8's axum bullet (line 551): "`fw-http` depends on `hyper` without features and on
  `hyper-util` with `tokio` for that" becomes `Embed::take_upgrade`, defaulted to `None`, which
  axum's adapter overrides; §3.8's limits table, `upgrades` row (line 637), is unchanged.
- §11's SPI table: a row for `Embed::take_upgrade`, `Upgraded::from_tokio` and the `tokio-io`
  feature.
- The CI check of stage f: `cargo test --workspace` and `cargo check --workspace` unify
  `tokio-io` on for `fw-http`, so the feature-off build is compiled only by a run selecting
  `fw-http` alone (`cargo check -p ulo-http --all-targets`), which the stage-f job needs as its own
  step.

## Needs sign-off

### S1. `Embed::take_upgrade` in place of a `hyper` feature on `ulo-http`

Decision 3: hyper's own tokio dependency made any hyper type in `ulo-http` a tokio path, and the
tower path's one hyper use moves to axum's adapter through a defaulted trait function. A tower host
declaring `upgrades` that leaves the default hands the app no upgrade, which its conformance run
reports. The alternative is a `hyper` feature on `ulo-http` keeping the old code, enabled by axum.

### S2. TLS ends at `Arc<rustls::ServerConfig>`, and `Backend::bind` takes it

Decision 5, option (b): the SPI's `tls` parameter changes type, `Tls::load` answers a
configuration, and the acceptor is built in the runtime crate. A backend outside the workspace
changes one line. The alternative is option (a), `futures-rustls`'s acceptor in `ulo-net` with
tokio consumers converting through `compat`.

### S3. One `Upgraded` over `futures-io`, two `Compat`s for a tokio-built one read through tokio

Decision 1: both directions are `tokio-util`'s code, at the cost of a buffer initialization per
read on that path. The alternatives are an `Upgraded` holding either flavour in an enum, which
writes the forwarding twice, or hand-written tokio impls.

### S4. `Upgraded::new` keeps its name with a `futures-io` bound

A caller outside the workspace passing tokio I/O to `new` fails on the trait bound, and the
error does not name `from_tokio`. The alternatives are `from_futures` beside `from_tokio` with
`new` removed, or `#[diagnostic::on_unimplemented]` on a local trait naming `from_tokio`.

### S5. `ulo-graphql-http`'s tree check passes at stage d, not b

F362: every tokio path in `ulo-graphql-http`'s tree starts at `ulo-ws`, reached through
`ulo-graphql-ws`, a plain dependency used only for subscriptions; the path through `ulo-http` exists
only because `ulo-ws` enables `tokio-io`. Stage d clears it with `ulo-ws`'s move. The alternative
is a default-on `subscriptions` feature in `ulo-graphql-http`, which also takes the WebSocket
transport out of an app without subscriptions.

### S6. Test certificates in `ulo-net/tests/fixtures`, read across crates

Decision 6: one CA and leaf, valid until 2126, read by three crates' tests through a relative
path. The alternatives are a copy per crate, or `rcgen` as a dev-dependency generating them per
run, which adds a crate to the lock.

## Verification

Full, unfiltered output of every run is in the session scratchpad, `runtime-b/`.

- **The tree checks,** first against a violation (`tree/violation.txt`): with
  `default = ["tokio-io"]` added to `ulo-http`'s features and `tokio-rustls` to `ulo-net`'s
  dependencies, `cargo tree -p ulo-http -e normal -i tokio` printed tokio through `tokio-rustls`,
  `ulo-net`, `tokio-util` and `ulo-http` itself, and `ulo-net`'s printed it through `tokio-rustls`.
  Both manifests and `Cargo.lock` were restored from copies, checked by hash. On the final tree
  (`tree/final.txt`) `ulo-http`'s prints nothing on stdout and exits 0 with "nothing to print" on
  stderr, tokio being a dev-dependency of `ulo-http`; `ulo-net`'s
  prints nothing and exits 101, "did not match any packages". With `--features tokio-io`,
  `ulo-http`'s prints tokio through `tokio-util` and itself. `ulo-graphql-http`'s still prints
  tokio, every path starting at `ulo-ws` (S5, F362).
- `cargo check --workspace --all-targets --all-features` with
  `CFLAGS=-I/opt/homebrew/opt/openssl/include LDFLAGS=-L/opt/homebrew/opt/openssl/lib`: exit 0, the
  17 known warnings in `crates/ulo/src` and no warning location elsewhere.
- `cargo check --workspace --all-targets`: exit 0, the same 17. `cargo check -p <crate>
  --all-targets` alone for `ulo-http`, `ulo-net`, `ulo-http-actix` and `ulo-graphql-http`: each
  exit 0, `ulo-http`'s being the one build of it with `tokio-io` off.
- `cargo +1.88 check --workspace --all-targets --exclude ulo-http-salvo --exclude
  ulo-graphql-async-graphql`: exit 0, the same 17 and no other.
- `cargo test --workspace --no-fail-fast` with the OpenSSL flags: 496 passed, 0 failed, 65 ignored
  across 129 test binaries: stage a's 463 and the 33 added, three in `tests/upgraded.rs`, three in
  `ulo-net`'s `tests/tls.rs`, two in `ulo-http-hyper`'s and 25 in `conformance_tls.rs`. Every HTTP
  host's conformance suite ran in it: actix 34 (4 ignored), axum 38, hyper 36 (2 ignored), poem
  38, rocket 38, salvo 39.
- `cargo test -p ulo-http-actix --features conformance-http2 --test conformance_http2 --locked`: 38
  passed. `cargo test -p ulo-rpc-tcp --test conformance --locked`: 25 passed.
- `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --lib -p <crate>` for `ulo-http`, `ulo-net`,
  `ulo-hyper-serve`, `ulo-http-hyper`, the five adapters, `ulo-http-conformance`, `ulo-ws`,
  `ulo-grpc` and `ulo-rpc-tcp`, and for `ulo-http` again with `--features tokio-io`: each passes.
- `cargo +1.98.1 clippy` over the twelve touched crates with `--all-targets --no-deps --locked`:
  exit 0, 52 warning locations. A checker reading `git diff -U0`'s hunks and the untracked files,
  run first against four planted locations (two on changed or new lines, reported; two on unchanged
  lines, not), reported one on this stage's code: `err_expect` at `ulo-net`'s `tests/tls.rs:70`,
  now `expect_err`. Run again on `ulo-net` after the fix, it reported none. No generated code
  changed, so `ulo-macro-lints` was not run.
- After the workspace test run, `Upgraded`'s doc lost its link to `from_tokio`, which does not
  resolve with the feature off, and `ulo-net`'s test took the `expect_err` fix; nothing else
  changed. `cargo test -p ulo-http --test upgraded` (1, and 3 with `tokio-io`), `cargo test -p
  ulo-net --test tls` (3), both `ulo-http` doc builds and `cargo +1.88 check -p ulo-net
  --all-targets` passed afterwards.
- No container was started, and no broker suite was run: no broker link changed. `docker ps` before
  and after listed the user's four, seaweedfs, mailpit, postgres:18 and redis:7, and nothing else.
