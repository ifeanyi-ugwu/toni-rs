# Divergences: race 2b, tests batch 21, `TaskEnd::Panicked` carrying the panic's message with a drop-time panic logged at `warn` and F374 found and fixed (S6), `.runtime(r)` and `.timer(t)` composing (S4), the runtime interface's three documentation items (S2, S3, S5), `Upgraded::from_futures` beside `from_tokio` (S4), `take_upgrade`'s sentence with the WebSocket hand-off's `upgrades` tested on two embed hosts and F375 filed (S1), and generated test certificates in `ulo-test-certs` (S6)

The thirty-ninth and fortieth responses, signed off 2026-10-10, answer `runtime-a.md` and
`runtime-b.md`. A panic ends a task as `TaskEnd::Panicked(Redacted)`, its message converted by the
core's `redact_panic`; the core's task wrapper now also drops the task's future inside
`catch_unwind`, so a panic raised while an aborted future is dropped reads `Aborted` and is logged at
`warn`. Building that found F374: on smol such a panic aborted the process. `.runtime(r)` sets the
spawning and the clock, `.timer(t)` the clock alone, each in the order written, so
`.runtime(r).timer(t)` binds `dyn Runtime` as `r`'s spawning with `t`'s clock and the app has one
clock whatever reads it. `spawn_with`, `ValueHandle` and `TaskHandle` carry the directed sentences;
`TaskSet`'s and both override hints already did, and tests now read the hints. `Upgraded::new` is
gone for `from_futures`. `Embed::take_upgrade` carries the sentence, qualified for the hosts that
convert on the `respond` path. The WebSocket suite reads no `upgrades` and runs on no embed host
(F375), so a gateway is tested inside axum and refused inside actix. Every TLS test takes its
certificate from `ulo-test-certs`, which `rcgen` fills when a test asks, and the committed fixtures
are deleted.

Files changed: the workspace `Cargo.toml` and `Cargo.lock`; `crates/ulo/{Cargo.toml,
src/runtime.rs, src/redact.rs, src/app/mod.rs, src/app/handle.rs, src/graph/wire.rs,
src/testing.rs}`; `crates/ulo-runtime-conformance/{Cargo.toml, src/lib.rs, src/cases/mod.rs,
src/cases/task.rs, src/cases/value.rs, src/cases/set.rs, src/cases/app.rs}`;
`crates/ulo-tokio/tests/{app.rs, handle.rs}`; `crates/ulo-http/{src/request.rs, src/embed.rs,
tests/upgraded.rs}`; `crates/ulo-http-axum/{Cargo.toml, tests/websocket.rs (new)}`;
`crates/ulo-http-actix/{Cargo.toml, tests/websocket.rs (new)}`; `crates/ulo-test-certs/` (new:
`Cargo.toml`, `src/lib.rs`, `tests/certs.rs`); `crates/ulo-net/{Cargo.toml, tests/tls.rs}` with
`tests/fixtures/{ca.pem, localhost.pem, localhost-key.pem}` deleted;
`crates/ulo-http-hyper/{Cargo.toml, tests/tls.rs}`; `crates/ulo-rpc-tcp/{Cargo.toml,
tests/conformance_tls.rs, tests/late_goaway.rs}`; F374 and F375 filed in the workspace's
`FRAMEWORK_GAPS.md`. The tree is `a23683ff` plus this batch; `a23683ff`, committed during the
build, touches `transports/RESPONSE.md` alone, and the batch started from `ec32eef4`.

## The signatures

```rust
// ulo, changed
#[derive(Debug)]                       // was Clone, Copy, Debug, PartialEq, Eq, Hash
pub enum TaskEnd {
    Finished,
    Aborted,
    Panicked(Redacted),                // was `Panicked`
}

// ulo::AppBuilder and ulo::testing::TestApp, signatures unchanged, meaning changed
pub fn timer(self, timer: impl Timer) -> Self;       // the clock alone; after `.runtime(r)`, `r`'s spawning stays
pub fn runtime(self, runtime: impl Runtime) -> Self; // the spawning and the clock

// ulo_http, changed
impl Upgraded {
    pub fn from_futures(io: impl futures_io::AsyncRead + futures_io::AsyncWrite + Send + Unpin + 'static) -> Self; // was `new`
    #[cfg(feature = "tokio-io")]
    pub fn from_tokio(io: impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin + 'static) -> Self;     // unchanged
}

// ulo_test_certs (publish = false), new
pub struct Certified { /* private: certificate PEM, key PEM, certificate DER */ }
impl Certified {
    pub fn self_signed(names: &[&str]) -> Certified;
    pub fn cert_pem(&self) -> &str;
    pub fn key_pem(&self) -> &str;                    // PKCS#8
    pub fn cert_der(&self) -> &CertificateDer<'static>;
    pub fn roots(&self) -> rustls::RootCertStore;     // trusting this certificate alone
}
pub fn localhost() -> &'static Certified;            // `localhost` and `127.0.0.1`, once per test binary
pub struct Ca { /* private: rcgen's `CertifiedIssuer` */ }
impl Ca {
    pub fn new() -> Ca;
    pub fn issue(&self, names: &[&str]) -> Certified; // a server's certificate, or a client's
    pub fn cert_pem(&self) -> String;
    pub fn roots(&self) -> rustls::RootCertStore;     // trusting this CA
}
impl Default for Ca {}
```

`ulo_runtime_conformance::runtime_suite!` stamps fourteen scenarios where it stamped twelve. No
handler, module or controller signature changes, and no macro's output changes. `Embed::take_upgrade`,
`TaskSet`, `spawn_with` and `ValueHandle` keep their signatures and gain or keep documentation
(decision 5).

What changes in meaning without a signature: `Dep<dyn Runtime>` and `AppHandle::runtime()` on an app
built `.runtime(r).timer(t)` answer an object that spawns on `r` and sleeps and reads the time on
`t`, where they answered nothing; `Mounted::timer()` and `Mounted::runtime()` follow it. A panic
raised while a task's future is dropped no longer reaches the runtime.

Private: in `ulo`, `runtime::{Clocked, Launched, Recorded, Kept, drop_caught}`,
`Redacted::copy_panic`, and `AppConfig`'s `spawner` field; `TaskHandle` holds an
`Arc<Mutex<Option<Recorded>>>` where it held an `AtomicU8`.

Dependencies: `ulo` gains `tracing`, runtime-free and already in every transport crate's tree
(S3). `ulo-runtime-conformance` gains `tracing`. `ulo-test-certs` depends on `rcgen` 0.14.10
(`crypto`, `pem`, `ring`; rust-version 1.88) and the workspace's `rustls`; the lock gains `rcgen`,
`yasna` and `pem`. `ulo-net`, `ulo-http-hyper` and `ulo-rpc-tcp` take `ulo-test-certs` as a
dev-dependency; `ulo-http-axum` takes `ulo-tokio`, `ulo-ws`, `futures-util`, `serde_json` and
`tokio-tungstenite` (`handshake`) as dev-dependencies, `ulo-http-actix` `ulo-tokio` and `ulo-ws`.

## Decisions

### 1. S6: `Panicked(Redacted)` through `redact_panic`, and the answer kept as a copy

- **The conversion:** `crate::redact::redact_panic(secrets, payload)` is the one `PanicRecovered`'s
  pipeline (`transport/pipeline.rs:175`), the lifecycle runner's `Outcome::Panicked`
  (`lifecycle/run.rs:95`) and `FailureReason::Panicked` at shutdown (`lifecycle/shutdown.rs:331`)
  call. The task wrapper calls it on the payload `CatchUnwind` answers; no second conversion exists.
- **The secrets:** `Spawn::spawn` receives a future and nothing of the graph, so the wrapper passes
  `SecretRegistry::default()`: the userinfo strip applies and no registered secret is replaced. The
  variant's doc says so. See S2.
- **Recorded, then kept:** the wrapper records `Finished` or `Panicked(Redacted)` in an
  `Arc<Mutex<Option<Recorded>>>` before it completes, the mutex taking the `AtomicU8`'s
  release/acquire role. The handle moves the record into its own `Kept` once `poll_ended` is
  `Ready`.
- **Answering more than once:** `TaskHandle` answers the same end each time it is polled, and
  `Redacted` is not `Clone`, its original being a `BoxError`. `Redacted::copy_panic` builds a second
  `Redacted` over the same `PanicMessage` and the same redacted text; every answer, the first
  included, is such a copy. `TaskEnd` therefore loses `Clone`, `Copy`, `PartialEq`, `Eq` and `Hash`,
  and every comparison in the workspace is a `matches!`. See S1.

### 2. S6: a panic while the future is dropped is caught by the core and logged at `warn`

- **Where it is caught:** `TaskHandle::launch` hands the runtime a `Launched`, a hand-written future
  holding the task's future under `CatchUnwind` (`runtime.rs:205`). Completing, it drops the future
  through `drop_caught` before recording the end; its own `Drop`, which an abort or a runtime
  dropping the task runs, does the same with nothing recorded, so the handle answers `Aborted`.
  `drop_caught` drops inside `catch_unwind` and logs a panic's message, through `redact_panic`, with
  `tracing::warn!` (`runtime.rs:241`).
- **Why in the core:** `poll_ended` reports only that the task is over, so no runtime adapter could
  report a drop-time panic through the handle. Caught by the core, it reads the same on every
  runtime and never reaches one.
- **F374, found by a probe:** at `ec32eef4` an abort dropped the future outside any `catch_unwind`.
  tokio caught the panic in its own cancel and the handle answered `Aborted`, the decision stage a
  recorded. On smol, `async-task` drops a cancelled future under its abort-on-unwind guard, and the
  probe's test binary ended at `async-task-4.7.1/src/utils.rs:17`, "aborting the process"
  (`probes/f374-smol-drop-panic-at-head.txt`, run on a scratch worktree at `ec32eef4`). The break
  restoring the uncaught drop does the same against the new scenario.
- **A panic in the drop of a future that already ended** is caught and logged the same way, and the
  recorded end stands. See S6.
- **The logger:** the core had no logging. `tracing` is the workspace's, runtime-free, and in every
  transport crate already. See S3.

### 3. The scenario reads the `warn` through a global subscriber

- **Deterministic:** the handle answers only once `poll_ended` is `Ready`, after the future's drop
  and so after the event. The scenario reads the record once the handle has answered, and asserts
  exactly one `warn` carrying the drop's unique message.
- **Global, not thread-local:** an aborted future is dropped on a thread the runtime picks, so a
  `with_default` subscriber on the test's thread would miss the event. `Recorder::installed()` sets a
  recording subscriber as the global default once per test binary through a `OnceLock`, and fails the
  scenario if another was installed first (`cases/mod.rs`). See S4.
- **Two more scenarios:** `a_panic_is_panicked_and_goes_no_further` and `spawn_with_answers_a_panic`
  panic with a message carrying `postgres://suite:hunter2@db.invalid/app` and assert the text arrives
  with the userinfo replaced; the first polls the handle twice and asserts both answers.
  `spawn_with_answers_finished_once_the_value_is_taken` pins S2's documented answer.

### 4. S4: `.runtime(r)` and `.timer(t)` compose, and `dyn Runtime` follows the clock

- **The builder:** `AppConfig` keeps the runtime as given in a private `spawner`. `.runtime(r)` sets
  `spawner`, `runtime` and `timer`, all `r`. `.timer(t)` sets `timer` to `t` and, when a `spawner`
  exists, `runtime` to `Clocked { spawner, timer: t }`, a private `Runtime` whose `Spawn` half
  delegates to the spawner and whose `Timer` half to `t` (`runtime.rs:39`, `app/mod.rs:132-155`).
  Each call overrides what it sets: `.runtime(r).timer(t)` spawns on `r` and times with `t`;
  `.timer(t).runtime(r)` is `r` for both; `.runtime(r).timer(t1).timer(t2)` composes `r` with `t2`,
  never with an earlier composition. `TestApp` delegates to the same builder.
- **The decision the brief asked for:** `dyn Runtime`'s `Timer` half returns `t`'s clock, not
  `r`'s. A transport sleeping through `mounted.runtime()` and the core timing through `config.timer`
  then read one clock, and a test's fake clock governs the transports' timeouts as well as the
  hooks. The cost is identity: `Dep<dyn Timer>` is `t` and `Dep<dyn Runtime>` the composite, two
  objects that agree, where `.runtime(r)` alone still binds one. See S5.
- **Who reads what:** `Dep<dyn Timer>` and `AppHandle::timer()` are `t`; `Dep<dyn Runtime>` and
  `AppHandle::runtime()` are the composite. The core's waits (hooks, constructions, readiness, the
  drain, the shutdown cap) read `config.timer`, the object `AppHandle::timer()` answers
  (`app/shared.rs`, `lifecycle/connect.rs`, `lifecycle/shutdown.rs`); `Mounted::timer()` is the
  runtime upcast (`transport/server.rs:266`), which is now the composite and so `t`'s clock. No
  transport changed: every clock it reads is `t`'s.
- **The binding:** `add_timer_module` binds `config.timer` under `dyn Timer` and `config.runtime`
  under `dyn Runtime` as before; its label stays `AppBuilder::runtime` when a runtime is set.

### 5. S2, S3 and S5: documentation, and the two hints read by tests

- **S2:** `spawn_with`'s doc ends "Polling after the value was taken answers
  `Err(TaskEnd::Finished)`", and `ValueHandle`'s sentence is replaced by the same one.
- **S3:** `TaskHandle`'s doc states the asymmetry in one sentence, "Dropping the handle detaches the
  task, which runs to its end, whereas dropping a `ulo_transport::TaskSet` aborts every task in it",
  replacing two. `TaskSet`'s doc already stated it in one sentence and is unchanged.
- **S5:** both hints already named the test builder, "help: set it with `TestApp::timer(..)`" and
  "help: set it with `TestApp::runtime(..)`" (`error/wiring.rs:528`, `:533`). No test read either;
  `overriding_the_runtime_is_refused_naming_the_test_builder` and
  `overriding_the_timer_is_refused_naming_the_test_builder` now assert the rendered text.

### 6. S4 (fortieth): `from_futures` and `from_tokio`, `new` removed

- `Upgraded::from_futures` takes what `new` took; `from_tokio`, behind `tokio-io`, wraps in
  `tokio-util`'s `Compat` and calls `from_futures`. The type's doc names both constructors.
- **Callers:** `new`'s only callers were `from_tokio` and `crates/ulo-http/tests/upgraded.rs`
  (two). Every other construction in the workspace was already `from_tokio`: the hyper backend, the
  axum, salvo, poem and rocket adapters, `ulo-ws-hyper` and the WebSocket tests.
- **No test pins the absence of `new`:** restoring it compiles, and every caller the workspace has
  names `from_futures` or `from_tokio`. The rename is pinned by `tests/upgraded.rs` failing to
  compile under the break that names it `new` again.

### 7. S1 (fortieth): the sentence, and what each suite asserts

- **The sentence, qualified:** "A host declaring `upgrades: true` must implement this, unless it
  builds the app's request itself for `Service::respond` and sets `upgrade` there"
  (`embed.rs:101-103`). The response's unqualified form is false for salvo, poem and rocket, which
  declare `upgrades`, keep the default, and set `Request::upgrade` on the `respond` path
  (`ulo-http-salvo/src/handler.rs:67`, `ulo-http-poem/src/endpoint.rs:33-56`,
  `ulo-http-rocket/src/handler.rs:68`). See S7.
- **The HTTP suite asserts both directions:** `upgrade::echo` requires a 101 and an echoed frame
  where the host declares `upgrades` and no 101 where it does not
  (`ulo-http-conformance/src/cases/upgrade.rs:19-26`). With axum's override replaced by the default
  both modes fail; with actix declaring `upgrades(true)` both modes fail (the table below).
- **The WebSocket suite asserts neither:** its `Host` has no limits and its two hosts are the
  standalone server and `ulo_http_hyper::Server`, a `Backend`, so `take_upgrade` is out of its
  reach. Filed as F375.
- **Added instead:** `crates/ulo-http-axum/tests/websocket.rs` serves a gateway through `WsModule`'s
  hand-off inside axum and requires a 101 and an answered message;
  `crates/ulo-http-actix/tests/websocket.rs` requires `listen()` to refuse the same app inside actix
  with `check_limits`'s sentence. See S8.

### 8. S6 (fortieth): `ulo-test-certs`, a `publish = false` crate

- **Why a crate and not a `test-util` feature on `ulo-net`:** a feature cannot enable a
  dev-dependency, so `rcgen` would become an optional normal dependency in `ulo-net`'s published
  manifest and the helper part of its published API; `cargo test --workspace` would unify the
  feature on for `ulo-net`'s normal build in every member's test run. A crate keeps `rcgen` out of
  every published manifest and follows `ulo-runtime-conformance` and `ulo-rpc-conformance`, whose
  path dev-dependencies Cargo strips when publishing. See S9.
- **What it holds:** `localhost()`, one self-signed certificate for `localhost` and `127.0.0.1` per
  test binary, made on first use; `Certified::self_signed(names)` for another, as the mismatched-key
  test needs; and `Ca`, a CA certificate and key that issue a server's or a client's certificate,
  for a test that verifies a client. No test in the workspace verifies a client yet;
  `tests/certs.rs` shows the CA through one handshake with client verification over buffers, and
  the self-signed certificate through one for each of its two names.
- **The anchor:** the replaced tests trusted a test CA that signed the leaf, since webpki refuses a
  CA certificate used as an end entity (`runtime-b.md` decision 6). A self-signed certificate that
  is not a CA is accepted as its own trust anchor, so `Certified::roots()` trusts the certificate
  itself and the server tests need no chain. See S10.
- **Users replaced:** every user of `crates/ulo-net/tests/fixtures/` found by searching for the
  directory, `include_bytes!`, `from_pem` and `BEGIN CERTIFICATE`: `ulo-net`'s `tests/tls.rs`,
  `ulo-http-hyper`'s `tests/tls.rs`, `ulo-rpc-tcp`'s `tests/conformance_tls.rs` and
  `tests/late_goaway.rs`. The three fixture files are deleted, and the search finds none left.
- **`rcgen`:** 0.14.10 declares `rust-version = "1.88"`, the workspace's floor, and the `+1.88` check
  builds it. It is reached only through dev-dependency edges: `cargo tree --workspace -i rcgen -e
  normal,dev` shows `ulo-test-certs` under `[dev-dependencies]` of `ulo-net`, `ulo-http-hyper` and
  `ulo-rpc-tcp` alone, and `cargo tree -p ulo-net -e normal -i rcgen` prints nothing, with and
  without `--all-features`.

## The tests

- **`ulo-runtime-conformance`, fourteen scenarios** on tokio and on smol:
  - `a_panic_while_an_aborted_future_drops_is_aborted_and_logged` (new): a task holding a value that
    panics in its drop, started and aborted; the end is `Aborted`, and the recorder holds exactly one
    `warn` carrying the panic's message.
  - `spawn_with_answers_finished_once_the_value_is_taken` (new): `Ok(7)`, then `Err(Finished)`.
  - `a_panic_is_panicked_and_goes_no_further` and `spawn_with_answers_a_panic` (changed): the end is
    `Panicked` whose text carries the message with `postgres://[redacted]@db.invalid/app` and not the
    password; the first asserts the same of a second poll.
  - The other ten compare with `matches!`, their assertions otherwise unchanged.
- **`crates/ulo-tokio/tests/app.rs`, fourteen tests** (nine before):
  - `a_timer_after_a_runtime_replaces_only_its_clock` and
    `a_test_s_timer_after_its_runtime_replaces_only_its_clock`, on `App::builder` and `TestApp`,
    with `Frozen`, a clock standing a day ahead whose sleeps end at once: `Dep<dyn Timer>`,
    `Dep<dyn Runtime>`, `AppHandle::timer()` and `AppHandle::runtime()` all read `Frozen`'s instant;
    each `AppHandle` accessor is its `Dep`; `Dep<dyn Runtime>`'s hour-long sleep ends within five
    seconds; a task spawned through it is `Finished`.
  - `a_runtime_after_a_timer_replaces_both` and `a_test_s_runtime_after_its_timer_replaces_both`:
    `Dep<dyn Timer>`, `Dep<dyn Runtime>` and both accessors are one object, which does not read
    `Frozen`'s instant.
  - `the_app_s_waits_read_the_timer_set_after_the_runtime`: an init hook that never returns, under
    an hour's `hook_timeout`, fails `connect` with `TimedOut` within five seconds.
  - `a_server_times_with_the_timer_set_after_the_runtime`: a server's `Mounted::timer()` and
    `Mounted::runtime()` both read `Frozen`'s instant in `prepare`.
  - `overriding_the_runtime_is_refused_naming_the_test_builder` (renamed from
    `overriding_the_runtime_is_refused`) and `overriding_the_timer_is_refused_naming_the_test_builder`
    (new): the variant and the rendered hint.
  - Removed: `a_timer_after_a_runtime_replaces_it` and `a_runtime_after_a_timer_replaces_it`, the
    last-call-wins pair.
- **`crates/ulo-http-axum/tests/websocket.rs`,** `a_gateway_on_the_http_transport_answers_inside_axum`.
- **`crates/ulo-http-actix/tests/websocket.rs`,**
  `a_gateway_on_the_http_transport_is_refused_by_a_host_without_upgrades`.
- **`crates/ulo-test-certs/tests/certs.rs`,**
  `a_client_trusting_the_localhost_certificate_completes_a_handshake_with_it` (both names) and
  `a_server_verifies_a_client_certificate_its_ca_issued` (the server holds the client's certificate
  afterwards).
- **The four TLS files** run as before on generated certificates: `ulo-net` 3, `ulo-http-hyper` 2,
  `conformance_tls` 27, `late_goaway` 1.

### Before and after

Each break went through `batch21/scripts/brk.py`, `batch20`'s script: it replaces each span
(asserting it occurs once), runs the target, writes every file back byte for byte with `Cargo.lock`
among them, and compares a hash over `git diff HEAD` of `crates`, `Cargo.toml` and `Cargo.lock` and
every untracked file under `crates`. Every restore reported `ok`, the hash `069b5bbd76e6` each time.
Specs are in `batch21/specs/`, full output in `batch21/broken/`.

| Break | Target | Result |
| --- | --- | --- |
| the payload replaced by `"a panic"` before conversion | runtime suite, tokio and smol | the two panic scenarios failed on each, "the panic's message, redacted, did not arrive: \"a panic\""; twelve passed |
| the message stored unredacted, its text the raw payload | runtime suite, tokio | the two panic scenarios failed, the text carrying `suite:hunter2@` |
| `copy_panic` giving the unredacted message as the text | same | the two panic scenarios failed the same way |
| the future dropped outside `catch_unwind`, as at `ec32eef4` | runtime suite, tokio | the drop-panic scenario failed, "logged at `warn` 0 times"; thirteen passed |
| the same | runtime suite, smol | the test binary aborted at `async-task-4.7.1/src/utils.rs:17`, "aborting the process" (F374) |
| the drop-time panic logged at `info` | runtime suite, tokio | the drop-panic scenario failed, "logged at `warn` 0 times" |
| a second poll after `Ok` answering `Err(Aborted)` | runtime suite, tokio | `spawn_with_answers_finished_once_the_value_is_taken` failed, `Some(Err(Aborted))` |
| `.timer(t)` after `.runtime(r)` clearing the runtime, the old rule | `ulo-tokio --test app` | three failed: both orders' clock tests at `Dep<dyn Runtime>`, the server test at `listen()` |
| `.timer(t)` after `.runtime(r)` keeping `r` as `dyn Runtime` | same | three failed: `Dep<dyn Runtime>`'s clock is not `Frozen`'s, on both builders and in the server's `prepare` |
| `.timer(t)` after `.runtime(r)` leaving `r` as the app's timer | same | three failed: `Dep<dyn Timer>` on both builders, and the hook test, "the init hook ran on the runtime's own clock" after 5 s |
| `.runtime(r)` after `.timer(t)` keeping `t` | same | the two "replaces both" tests failed, `Dep<dyn Timer>` another object |
| the composite sleeping on the spawner's clock | same | both clock tests failed after 5 s, "slept an hour on the runtime's own clock" |
| the runtime override's hint replaced | same | the runtime hint test failed |
| the timer override's hint replaced | same | the timer hint test failed |
| `from_futures` named `new` again | `ulo-http --features tokio-io --test upgraded` | E0599, no `from_futures` |
| axum's `take_upgrade` replaced by the default | `ulo-http-axum --test websocket` | failed, "did not switch protocols: HTTP error: 400 Bad Request" |
| the same | `ulo-http-axum --test conformance`, filtered to `upgrade` | `nested::` and `fallback::upgrade_echoes_a_frame` failed, "the host declares `upgrades` and did not switch protocols" |
| actix declaring `upgrades(true)` | `ulo-http-actix --test websocket` | failed, the app listened |
| the same | `ulo-http-actix --test conformance`, filtered to `upgrade` | both modes failed, "did not switch protocols" |
| `check_limits`'s `upgrades` refusal removed | `ulo-http-actix --test websocket` | failed, the app listened |
| `localhost()` without `127.0.0.1` | `ulo-test-certs` | the localhost test failed at the IP name's handshake |
| `Ca::issue` self-signing | same | the client-verification test failed at the handshake |
| `roots()` trusting nothing | `ulo-net --test tls` | the handshake test failed, the client refusing the certificate |
| the same | `ulo-http-hyper --test tls` | both failed at the TLS connect |
| the same | `ulo-rpc-tcp --test conformance_tls`, filtered to `unary_round_trip` | failed, the bridge's handshakes refused |
| the same | `ulo-rpc-tcp --test late_goaway` | failed, `InvalidCertificate(UnknownIssuer)` |
| the restored tree | every target above | every test passed |

## Left for the transports DESIGN fold

Line numbers are those read on `a23683ff`.

- §1's crate table: a row for `fw-test-certs`, `publish = false`, a dev-dependency of the crates
  whose tests serve TLS: `localhost()`, `Certified`, `Ca`, generated with `rcgen`. `fw-runtime-conformance`'s
  row (line 46): fourteen scenarios.
- §3.5, the `Upgraded` block (lines 543-548): `from_futures` in place of `new`.
- §3.8, `take_upgrade`'s line in the `Embed` block (line 587): "a host declaring `upgrades: true`
  implements it, unless it builds the request for `respond` and sets `upgrade` there".
- §8, the `TaskEnd` block (line 1186): `#[derive(Debug)] pub enum TaskEnd { Finished, Aborted,
  Panicked(Redacted) }`; `ValueHandle`'s comment (line 1205) reads "polling after the value was taken
  answers `Err(TaskEnd::Finished)`".
- §8, "The app" (line 1222): the composition rule of decision 4 in place of "the later call wins";
  `Dep<dyn Runtime>` after `.runtime(r).timer(t)` spawns on `r` and times with `t`.
- §8, "Conformance" (line 1237): fourteen scenarios; the panic scenarios assert the message,
  redacted, on a second poll too; the drop-panic scenario and its global recording subscriber;
  `spawn_with`'s second poll.
- §11, X27 (line 1317): `Upgraded::from_futures` in place of `Upgraded::new`.
- Decision 47 (line 1468): "`TaskEnd::Panicked` carries no payload ... reads `Aborted`, `poll_ended`'s
  `()` carrying nothing a runtime saw" is superseded: `Panicked(Redacted)` through `redact_panic`
  with the userinfo strip alone, and a drop-time panic caught by the core's wrapper, read `Aborted`
  and logged at `warn` (F374 for what smol did without it).
- Decision 48 (line 1469) is superseded by the thirty-ninth response's S4 as decision 4 builds it.
- Decision 50 (line 1471): "`Upgraded::new` keeps its name ..." is superseded by the two named
  constructors.
- §4.1, the conformance paragraph (line 757), or F375's own line: the WebSocket suite runs on no
  embed host; the hand-off's `upgrades` is tested inside axum and actix.

And in the core DESIGN, `docs/experiments/di-redesign/DESIGN.md`:

- §3.9 (line 388): "`dyn Timer` is then the same object, upcast" holds for `.runtime(r)` alone;
  after `.runtime(r).timer(t)`, `dyn Timer` is `t` and `dyn Runtime` is `r`'s spawning with `t`'s
  clock. The paragraph gains that the core logs at `warn` through `tracing` for a panic raised while
  a task's future is dropped, its only log.
- §9.4 (line 893): "The later call wins ..." is replaced by: `.runtime(r)` sets the spawning and the
  clock, `.timer(t)` the clock alone, each overriding only what it sets, in order.
- §11's `Timer` bullet (line 1241): `TestApp::timer(..)` after `TestApp::runtime(..)` replaces only
  the clock, a real executor under a test's clock.

## Needs sign-off

Numbered from S1 for this batch; the items it builds are named by their logs.

### S1. `TaskEnd` loses `Clone`, `Copy`, `PartialEq`, `Eq` and `Hash`, and each answer is a copy

Decision 1. `Redacted` holds a `BoxError` and is not `Clone`, so a variant carrying it cannot derive
those; a handle answering the same end on every poll answers a copy built by `copy_panic`.
Comparisons are `matches!`. The alternatives are `Panicked(Arc<Redacted>)`, which keeps `Clone`, or a
`Clone` `Redacted` over an `Arc` original, which changes `into_inner`'s ownership for every core
error.

### S2. A task's panic message is scrubbed of userinfo and of no registered secret

Decision 1. `Spawn::spawn` sees no graph, so `redact_panic` runs with an empty registry. The
alternatives are a runtime the app wraps to carry its registry, which splits `Dep<dyn Runtime>` from
the object given, or a caller re-scrubbing through `AppHandle::redact`.

### S3. The core depends on `tracing` for one `warn`

Decision 2. A drop-time panic reaches no caller, so it is logged where it is caught, and `tracing`
is runtime-free and in every transport crate's tree. The alternatives are a hook a runtime adapter
or an app installs on the core, or writing to stderr.

### S4. The runtime suite installs a global `tracing` subscriber

Decision 3. A runtime drops an aborted future on a thread of its choosing; the subscriber is set
once per test binary and fails the scenario if another came first, so a runtime crate's harness
installs none. The alternative is leaving the `warn` untested by the suite and pinning it on one
runtime with a current-thread executor.

### S5. After `.runtime(r).timer(t)`, `dyn Runtime` is a composite timing with `t`

Decision 4. The app then has one clock whatever reads it, and `Dep<dyn Timer>` and
`Dep<dyn Runtime>` are two objects that agree; `.runtime(r)` alone still binds one object. The
alternative is `Dep<dyn Runtime>` staying `r`, its `Timer` half `r`'s clock, with transports that
sleep through the runtime on another clock than the core's waits.

### S6. A panic while dropping a future that already ended is logged and changes nothing

Decision 2. The end recorded from the poll stands, `Finished` or `Panicked`. The alternative is
`Panicked` with the drop's message.

### S7. `take_upgrade`'s sentence is qualified for the `respond` path

Decision 7. The response's sentence, unqualified, would be false for salvo, poem and rocket. The
alternative is moving their conversions into `take_upgrade`, which they cannot use on the `respond`
path, or the sentence as given.

### S8. The WebSocket hand-off's `upgrades` is tested by two adapter tests, not by the suite

Decision 7. The WebSocket suite's `Host` binds a server the app owns, and an embed host serves on
its own listener; F375 records the gap for salvo, poem and rocket. The alternative is a second
`Host` shape in the suite, built in its own batch.

### S9. The certificate helper is a `publish = false` crate

Decision 8. The alternative is a `test-util` feature on `ulo-net`, with `rcgen` an optional
dependency in its published manifest.

### S10. The server tests trust the self-signed certificate directly, and the CA serves client verification

Decision 8. One certificate per test binary, made on first use, its own trust anchor. The
alternatives are a CA-issued leaf for every server test, as the fixtures had, or a certificate per
test.

## Verification

Full, unfiltered output of every run is in the session scratchpad, `batch21/`: `verify/` for the
pass below, `runs/` for the runs during the build, `broken/` and `specs/` for the breaks, `probes/`
for F374's probe, and `scripts/`.

- **The listed crates,** `cargo test -p <crate> --locked` with the OpenSSL flags, once each on the
  final tree (`verify/crates-summary.txt`, `verify/crate-*.txt`):
  `ulo` 1 passed, 5 ignored; `ulo-transport` 1 and 1; `ulo-tokio` 34 and 1 (`app.rs` 14,
  the runtime suite 14, `handle.rs` 6); `ulo-smol` 14 and 1; `ulo-runtime-conformance` 0 and 1, its
  `ignore` example; `ulo-http` 20 and 9, with `--features tokio-io` 22 and 9; `ulo-http-hyper` 44
  and 3, its `tests/tls.rs` 2 among them; `ulo-http-axum` 39 and 1 (the suite 38, `websocket.rs` 1);
  `ulo-http-salvo` 39 and 2; `ulo-http-poem` 38 and 1; `ulo-http-actix` 35 and 5 (the suite 34,
  `websocket.rs` 1), and `--features conformance-http2 --test conformance_http2` 38; `ulo-http-rocket`
  38 and 1; `ulo-net` 5; `ulo-test-certs` 2 and 1; `ulo-ws` 49 and 4; `ulo-ws-hyper` 70 and 1;
  `ulo-ws-conformance` 1 and 2, the WebSocket suite's 29 scenarios passing on both hosts inside
  `ulo-ws` and `ulo-ws-hyper`; `ulo-rpc` 17 and 5; `ulo-rpc-tcp` 89 and 1 over 10 binaries, its
  three conformance suites, `conformance_tls` (27) and `late_goaway` among them;
  `ulo-codegen-tests` 53. Every run exited 0 and reported a nonzero count where the crate has tests.
- `cargo test --workspace --no-fail-fast` with the OpenSSL flags: 634 passed, 0 failed, 72 ignored
  across 165 test binaries (`verify/workspace-test.txt`): batch 20's 621 and thirteen added, four
  scenarios over two runtimes, five in `ulo-tokio`'s `app.rs` (fourteen where nine were), the two
  WebSocket tests and the two in `ulo-test-certs`. The ignored one added is `ulo-test-certs`' `ignore`
  doc example; the five binaries added are its three and the two `websocket.rs`.
- `cargo check --workspace --all-targets --all-features` with
  `CFLAGS=-I/opt/homebrew/opt/openssl/include LDFLAGS=-L/opt/homebrew/opt/openssl/lib`, and with
  defaults: exit 0, the 17 known warnings in `crates/ulo/src` and no other location
  (`scripts/diag.py` over the JSON messages; during the build it reported an unused `AppHandle`
  import in `ulo-tokio`'s `tests/app.rs`, since removed). `cargo check -p ulo-http --all-targets`,
  the one build with `tokio-io` off: the same.
- `cargo +1.88 check --workspace --all-targets --exclude ulo-http-salvo --exclude
  ulo-graphql-async-graphql`: exit 0, the same 17 and no other; `rcgen` 0.14.10 among the crates it
  built.
- **The tree checks,** `cargo tree -p <crate> -e normal -i tokio --locked` (`verify/tree.txt`):
  stdout is empty for `ulo`, `ulo-transport`, `ulo-net`, `ulo-http` (defaults), `ulo-rpc`, `ulo-ws`,
  `ulo-graphql-ws`, `ulo-graphql-http`, `ulo-ws-conformance` and `ulo-smol`, each exiting 101 with
  "did not match any packages" or 0 with "nothing to print". `ulo-ws-hyper` prints tokio through
  hyper, the positive control. `ulo-net`'s normal tree at default features lists no crate this
  batch added, and `-i rcgen` prints nothing with and without `--all-features`.
- `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --lib -p <crate>` for `ulo`,
  `ulo-runtime-conformance`, `ulo-http` (and again with `--features tokio-io`), `ulo-test-certs`,
  `ulo-net`, `ulo-tokio`, `ulo-http-axum`, `ulo-http-actix`, `ulo-http-hyper` and `ulo-rpc-tcp`:
  each exits 0. A broken link planted in `ulo-test-certs`' visible doc failed the check, "unresolved
  link to `NoSuchItem`" (`broken/doc-planted.txt`), and was restored by hash.
- `cargo +1.98.1 clippy -p ulo -p ulo-runtime-conformance -p ulo-tokio -p ulo-smol -p ulo-http -p
  ulo-http-axum -p ulo-http-actix -p ulo-net -p ulo-test-certs -p ulo-http-hyper -p ulo-rpc-tcp
  --all-targets --no-deps --locked`: exit 0, 99 warning locations, 25 outside `crates/ulo/src`.
  `scripts/changed_lines.py` over `git diff -U0 HEAD` and the untracked files reported one on this
  batch's lines, `redundant_closure` at `ulo-tokio`'s `tests/app.rs:160`, now the function itself;
  rerun on `ulo-tokio` it reported none. Given an existing location as a planted changed line, it
  reported it. After that fix `ulo-tokio --test app` passed again (14) and `cargo +1.88 check -p
  ulo-tokio --all-targets` exited 0; no other file changed after the workspace run but this one and
  the records.
- **Containers:** the user's four, seaweedfs, mailpit, postgres:18 and redis:7, before and after
  every run and untouched (`verify/docker-*.txt`). No broker suite ran and no container was
  started: no link changed. No permission check refused an action.
