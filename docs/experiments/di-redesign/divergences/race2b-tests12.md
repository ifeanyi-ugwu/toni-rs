# Divergences: race 2b, tests batch 12, the client's encoding failure pinned, the post-deadline cap on waited-on bodies, shadowed container ports and startup failures in files

The twenty-ninth response, signed off 2026-10-07, settled five builds. `RpcClient` answering a
request frame its link cannot encode `Internal` is pinned by a unit test. HTTP's 1 MiB cap on an
error handler's answer to a passed deadline bounds only a body the server waits on; a body ended on
the first poll is delivered whatever its size. `on_stream_end` firing for an untracked stream on
that path is recorded here. The broker startup flakes, F338 on NATS and F342 on Redis, have one
cause, reproduced and fixed: a host listener on `127.0.0.1` at the port Docker published the
container on receives the harness's connections. Every conformance startup failure is written to
a file, which CI prints and uploads when a job fails.

Files added: `crates/ulo-rpc-conformance/src/failures.rs`, `crates/ulo-http-conformance/src/failures.rs`,
this file.
Files changed: `.github/workflows/ci.yml`, `crates/ulo-rpc/src/client.rs`,
`crates/ulo-http/{src/service.rs, src/limits.rs, src/server.rs, src/backend.rs,
tests/post_deadline_body.rs}`, `crates/ulo-http-hyper/tests/route_timeout.rs`,
`crates/ulo-rpc-conformance/src/{lib.rs, relay.rs, cases/app.rs}`,
`crates/ulo-http-conformance/src/{lib.rs, reference.rs}`, the `tests/conformance.rs` of
`ulo-rpc-{nats, redis, mqtt, rabbitmq, kafka, tcp, udp}`, `crates/ulo-rpc-tcp/tests/conformance_cbor.rs`,
the `tests/conformance.rs` of `ulo-http-{axum, poem, rocket, salvo}`,
`crates/ulo-http-actix/tests/common/mod.rs`, and F338, F342 and the new F343 in the workspace's
`FRAMEWORK_GAPS.md`. The tree is `75757925` plus this batch.

## The signatures

```rust
// ulo_rpc_conformance::relay (new)
pub fn shadowed(addr: SocketAddr) -> bool;
pub async fn unshadowed<E, F, Fut>(what: &str, start: F) -> (E, Vec<SocketAddr>)
where F: FnMut() -> Fut, Fut: Future<Output = (E, Vec<SocketAddr>)>;

// ulo_rpc_conformance and ulo_http_conformance, each (new)
#[macro_export] macro_rules! startup_failed { ($($arg:tt)+) => { /* write, then panic */ } }
pub fn failures_dir() -> PathBuf;   // $ULO_CONFORMANCE_FAILURES, else <workspace>/target/conformance-failures
```

No public item of `ulo-rpc` or `ulo-http` changed. `ulo-rpc` gains its first unit-test module, in
`client.rs`. Private to `ulo-http`'s service: `completed` keeps `BUFFER_CAP` and `FRAMES_PER_POLL`
and gains the `waited` and `pending` flags.

## Decisions

### 1. S2 pinned: a request frame the link cannot encode is `Internal`

- **Written:** `client::tests::a_request_frame_the_link_cannot_encode_is_internal`, with a link
  declaring JSON whose client side encodes every frame with `Codec::Cbor.encode_frame`. The
  request's payload is `{"id":1}`, whose opening `{` is a CBOR text string of 27 bytes that seven
  bytes cannot fill, so the encoding fails with `FrameUnencodable` the way a link encoding its own
  frames wrongly fails. The test asserts `ErrorKind::Internal` and the codec's message.
- **Why a unit test:** `RpcClient::new` is crate-private, and the failure is the client's mapping
  alone; no link in the workspace encodes its request frames wrongly. The timer is one that never
  expires, since the call fails as it is sent.
- **`RpcError`'s doc** lists the mapping beside the others it names.

### 2. HTTP: the cap bounds only a body the server waits on

- **Written:** `completed` reads the body's first poll without the cap: up to `FRAMES_PER_POLL`
  (32) ready frames. A body whose end is reached in that poll, by `None`, by trailers or by
  `is_end_stream`, is buffered whatever its size. A body that returns `Pending` or is still ready
  after 32 frames is one the server waits on from then: the bytes already read count toward the
  cap, a body already past it answers the 504 at once, and every later frame is checked as before.
  The `size_hint` check before reading is gone, so a `Full` of 2 MiB is delivered, and no type is
  named. The over-cap 504 and its `warn` are unchanged.
- **Why the first poll is bounded by 32 frames:** the first poll has to end for a body whose frames
  are always ready and never end, so the grace's timer can fire on a current-thread runtime, which
  is what `FRAMES_PER_POLL` already did. A ready body of more than 32 frames is therefore held to
  the cap; see S1.
- **A pending body is no longer polled in a loop.** `completed` woke its own task on every poll,
  including one where the body had returned `Pending` and registered its waker. A body pending for
  300 ms was polled 18,865 times in a probe, and 4 times with the wake kept for a body still ready
  after 32 frames, the case it exists for. Found while moving the cap; predates this batch. See S2.
- **Docs:** `limits.rs` (`Timeout`), `server.rs` (`timeout_grace`), `backend.rs`
  (`HttpConfig::timeout_grace`) and the service's own comments state the two cases.

### 3. The tests of the cap

- **`post_deadline_body.rs`, through `AppService::call`:** the cap's boundary moved to waited-on
  bodies. `WaitedOnTimeout(len)` is a stream pending once, through `tokio::task::yield_now`, before
  chunks that are then all ready: exactly 1 MiB is written with its length, one byte more answers
  504. `a_stream_ended_on_the_first_poll_is_written_whatever_its_size` answers 2 MiB as four ready
  chunks of 512 KiB, a stream rather than a `Full`, so no type is special-cased.
  `a_pending_body_is_polled_when_it_wakes_the_reader` counts the polls of a body pending for
  100 ms and allows 8.
- **`route_timeout.rs`, through hyper:** `/full` answers a `Full` of 2 MiB, written whole with
  `content-length: 2097152` and no over-cap `warn`; `/large` became a waited-on body of
  1,114,112 bytes, still refused before the grace ends.

### 4. S5's log line

`on_stream_end` now fires for a stream an error handler answers a passed deadline with as
`Response::new(HttpBody::stream(..))`, never wrapped in `Tracked`, where it never fired before:
the buffered copy reports the end of every body of unknown length on this path, tracked or not.
Batch 11 built it (decision 4, last point); the response accepted it as one meaning everywhere.

### 5. The startup flakes: a host listener shadows the container's published port

- **How each start chooses ports.** No broker suite probes and releases a port. Docker publishes
  each container port at a host port it chooses, read back with `get_host_port_ipv4`; the relay
  binds `127.0.0.1:0` and keeps its listener; the app server of a broker link binds nothing. The
  HTTP suites hand the host a held listener or bind port 0 and read it back. The TCP and UDP suites
  do probe and release (F343, below).
- **The race.** OrbStack's dockerd picks a published port inside its VM, sequentially through
  32768–60999 (sampled: consecutive containers got 53003, 53004, 53005), without seeing macOS's
  sockets, and forwards it from a wildcard listener on macOS (`lsof`: `OrbStack *:53005`). macOS's
  ephemeral range is 49152–65535. A socket already listening on `127.0.0.1` at that port is the
  more specific match: a stand-in listener bound with `SO_REUSEADDR`, as tokio and libuv bind, on
  the port Docker was about to hand out received the connection made to the next container's
  published port, with OrbStack's wildcard listener on the same port beside it. The harness
  connects to `127.0.0.1:<port>`, so the scenario reaches that listener instead of its broker.
- **Reproduced.** This host runs VS Code helpers listening on `127.0.0.1:53011`, `57180`,
  `57391` and `58018`, each answering `HTTP/1.0 400 ... WebSockets request was expected`. The
  Redis suite looped 20 times, one run after another, failed once, on run 1, while Docker's
  counter covered 53008–53031: "the conformance server did not start: transport `Rpc` failed to
  bind: Incompatible type - Parse error at 1 / Unexpected `84` / Unexpected `72`", the bytes `T`
  and `H` of an HTTP response read as RESP. Runs 2–20 covered 53032–53487, where nothing listens on
  `127.0.0.1`, and passed. With the counter advanced by a container publishing a range of ports, a
  run with the detection logging and keeping the container failed across `127.0.0.1:57391` the same
  way, and one across `55854` (OrbStack's own `127.0.0.1` listener) and `55859` failed twice:
  "transport `Rpc` failed to bind: Connection reset by peer (os error 54)", F342's message, and
  "127.0.0.1:55859 did not accept a connection within 10s". The listener at `55859` was gone by
  then and is inferred to be another scenario's relay, the only listeners the suite opens. Full
  outputs were kept for every run.
- **NATS not reproduced:** a run across `58018` passed. The NATS image exposes three ports, so
  each container takes three from the counter, and `58018` went to `6222` or `8222`, which the
  suite never connects to. F338's 77.6 s fits a NATS client retrying against a listener that is not
  NATS. The cause is the same harness path, and the fix covers it.
- **Written:** `relay::unshadowed(what, start)` runs `start`, which starts a container and answers
  its loopback addresses, until `relay::shadowed` holds for none of them, at most five times,
  dropping the shadowed container and logging each retry on stderr; the fifth fails through
  `startup_failed!`. `shadowed` binds `127.0.0.1` at the port with `SO_REUSEADDR` for a moment: the
  engine's wildcard listener allows it and a specific listener refuses it, whichever process holds
  it. All five broker suites start their containers through it; Kafka checks both listeners and
  keeps its two relays across attempts, since they are bound before the container to be
  advertised. After a container's ports are published, neither a relay nor a VS Code helper can
  take one: an ephemeral bind skips a port a wildcard listener holds (2,000 of 2,000 binds did).
- **macOS only.** In a Linux container, a wildcard bind beside a `127.0.0.1` listener was refused
  (`EADDRINUSE`), so the shadow cannot form, and the probe itself was refused beside a wildcard
  listener, so it would report every container shadowed. Off macOS `shadowed` answers `false`.
- **Shown working:** the fixed build across `127.0.0.1:57180` logged one restart and passed 24 of
  24.
- **Considered:** a registry of the suite's own relay ports, which missed the editor's listener
  that the reproduction found; reaching containers at their bridge address, which Docker Desktop
  on macOS does not route; reaching them over `[::1]`, which depends on the engine publishing on
  IPv6 and was not checked on a GitHub runner.

### 6. Startup failures are written to files

- **Written:** `startup_failed!` in each conformance crate takes `panic!`'s arguments, writes the
  message to a file of its own under `failures_dir()`, and panics with the message and the file's
  path, at the caller's location (`#[track_caller]`). The file is named after the package, the
  test, the process id and the time, and starts with those and the binary's path. Every startup
  path fails through it: the broker containers' start, port mapping and `reachable`,
  `unshadowed`'s fifth attempt, the relay's bind, the TCP and UDP probes, the RPC server's
  `listen`, the client's wiring, connect and lookup, `ready`'s boot budget, an unexpected startup
  error in `refused_at_startup`, the HTTP suite app's wiring and connect, the reference host, and
  each HTTP host's listen, bind, adoption and rocket's lift-off.
- **The directory** is `$ULO_CONFORMANCE_FAILURES` when set, otherwise
  `<workspace>/target/conformance-failures`, from the conformance crate's manifest path, so a
  `CARGO_TARGET_DIR` elsewhere does not move it.
- **CI:** the `test` and `rpc conformance (brokers)` jobs each end with two `if: failure()` steps:
  one printing every file in the directory into the job log, one uploading it with
  `actions/upload-artifact@v4` (`conformance-failures-test`, `conformance-failures-brokers`,
  `if-no-files-found: ignore`). libtest already prints a failing test's captured output, the panic
  message included; the print step puts the message in the log a second time, from the file.
  `--nocapture` was not added: it interleaves every scenario's output and adds nothing a failing
  test's own capture does not carry.
- **Shown working:** the Redis suite's `unary_round_trip` with `DOCKER_HOST` pointing at a socket
  that does not exist failed at `crates/ulo-rpc-redis/tests/conformance.rs:35:41` with "the Redis
  container did not start: failed to initialize a docker client: Socket not found:
  /nonexistent/docker.sock (written to .../target/conformance-failures/ulo-rpc-redis-unary_round_trip-55951-1791389781118597000.txt)",
  and the file held the package, test, binary, time and that message.

## Unrelated gaps found

- **F343:** the TCP and UDP suites, and `deadline_answers.rs`, probe a port, release it and let the
  server bind it later; nothing failed this way, and the TCP link cannot adopt a held listener.
- `actionlint` reports SC2086 (info) at `ci.yml:118`, the unquoted `$pkgs` of the `msrv` job, which
  splits on purpose. Not changed.

## Left for the transports DESIGN fold

These DESIGN statements describe the cap before this batch: §2.4's deadline paragraph (line 238),
"a body that completes within the grace and within the 1 MiB the server buffers for it"; §3's
cancellation paragraph (line 495), the `size_hint` check before reading and "the cap applies to a
body of known length as to a stream", and the read's wake; §8's table row (line 1282) and decision
24 (line 1316), "within the 1 MiB buffered for it".

## Needs sign-off

### S1. The first poll is at most 32 frames

Decision 2: "end reached on the first poll" read as the first poll of `completed`, which takes at
most `FRAMES_PER_POLL` ready frames. A ready body of 33 frames or more and over 1 MiB answers 504.

### S2. A pending body wakes the reader itself

Decision 2: `completed` no longer wakes its own task when the body returned `Pending`. Not asked
for; pinned by `a_pending_body_is_polled_when_it_wakes_the_reader`.

### S3. `unshadowed`: a bind probe, macOS only, five attempts

Decision 5: the detection, its platform gate, the retry count, and the log on stderr.

### S4. Which failures `startup_failed!` covers

Decision 6: `ready`'s boot budget and an unexpected error in `refused_at_startup` count as startup
failures; a scenario's own assertions do not write files.

### S5. CI prints the files rather than running `--nocapture`

Decision 6.

## Verification

Each new test against the change undone, every file restored by `shasum -c`; full output of every
failing run kept:

| Undone | Test | Result |
| --- | --- | --- |
| `send_error`'s `FrameUnencodable` arm removed | `a_request_frame_the_link_cannot_encode_is_internal` | fails: "expected `Internal` for an unencodable request frame, got: RpcError { kind: Unavailable, message: \"the link refused the frame: the Cbor codec cannot encode the frame: Io(Error { kind: UnexpectedEof, .. })\" }" |
| the cap on every body (`waited` starting `true`) | `a_stream_ended_on_the_first_poll_is_written_whatever_its_size`; `a_full_body_over_the_buffer_..._is_written_whole` | the first: buffered length `Some(123)` against `Some(2097152)`, the 504's body; the second: `("HTTP/1.1 504 Gateway Timeout", 123)` against `("HTTP/1.1 503 Service Unavailable", 2097152)` |
| no cap (`waited` never set, no check after the first poll) | `a_waited_on_body_one_byte_over_the_cap_answers_504`; `a_waited_on_body_over_the_buffer_..._before_the_grace_ends` | the first answers 503; the second `HTTP/1.1 503 Service Unavailable` |
| the cap's `>` as `>=` | `a_waited_on_body_of_exactly_the_cap_is_written_with_its_length` | fails: `Some(123)` against `Some(1048576)` |
| the wake on every poll restored | `a_pending_body_is_polled_when_it_wakes_the_reader` | fails: "a body pending for 100ms was polled 6650 times while the server waited on it" |

The startup fix was shown by the reproduction and the run across `57180` in decision 5, and item 6
by the forced failure there.

Three consecutive runs per target on stable, one suite at a time, test time as libtest reports it;
the brokers with `--features integration`. No run logged a restart from `unshadowed`.

| Target | Run 1 | Run 2 | Run 3 | Result |
| --- | --- | --- | --- | --- |
| `ulo-rpc` lib | 0.00 s | 0.00 s | 0.00 s | 1 passed |
| `ulo-rpc-tcp` conformance | 1.31 s | 1.31 s | 1.31 s | 24 passed |
| `ulo-rpc-tcp` conformance_cbor | 1.31 s | 1.31 s | 1.31 s | 24 passed |
| `ulo-rpc-tcp` deadline_answers | 0.21 s | 0.20 s | 0.20 s | 2 passed |
| `ulo-rpc-udp` conformance | 1.31 s | 1.30 s | 1.31 s | 23 passed, 1 ignored |
| `ulo-rpc-nats` | 6.18 s | 4.04 s | 5.71 s | 24 passed |
| `ulo-rpc-redis` | 9.69 s | 5.49 s | 7.31 s | 24 passed |
| `ulo-rpc-mqtt` | 7.87 s | 11.63 s | 11.27 s | 24 passed |
| `ulo-rpc-rabbitmq` | 66.39 s | 63.35 s | 63.39 s | 24 passed |
| `ulo-rpc-kafka` | 25.69 s | 26.45 s | 22.49 s | 24 passed |
| `ulo-http-hyper` conformance | 0.32 s | 0.31 s | 0.31 s | 36 passed, 2 ignored |
| `ulo-http-hyper` route_timeout | 0.71 s | 0.71 s | 0.70 s | 6 passed |
| `ulo-http` post_deadline_body | 0.16 s | 0.16 s | 0.16 s | 6 passed |
| `ulo-http-axum` | 0.32 s | 0.31 s | 0.31 s | 38 passed |
| `ulo-http-actix` | 1.02 s | 1.02 s | 1.02 s | 34 passed, 4 ignored |
| `ulo-http-actix` conformance_http2 | 4.04 s | 4.04 s | 4.03 s | 38 passed |
| `ulo-http-poem` | 0.74 s | 0.73 s | 0.74 s | 38 passed |
| `ulo-http-rocket` | 4.03 s | 4.02 s | 4.02 s | 38 passed |
| `ulo-http-salvo` | 0.32 s | 0.31 s | 0.31 s | 39 passed |
| `ulo-codegen-tests` replies | 0.01 s | 0.00 s | 0.00 s | 12 passed |
| `ulo-codegen-tests` deadlines | 0.51 s | 0.51 s | 0.51 s | 10 passed |

RabbitMQ took 63–66 s against batch 11's 27–29 s; no restart was logged and no scenario failed, and
the suite starts its container as before plus one bind. F341 holds the open question on this
host's RabbitMQ timings. The Docker engine answered throughout; the user's `postgres:18`,
`redis:7`, `axllent/mailpit` and `chrislusf/seaweedfs` ran throughout and were not touched. The
containers that advanced Docker's port counter, `python:3-alpine` and `rhysd/actionlint` were
removed after use.

- `cargo check --workspace --all-targets --all-features` on stable, with
  `CFLAGS=-I/opt/homebrew/opt/openssl/include LDFLAGS=-L/opt/homebrew/opt/openssl/lib`, and
  `cargo +1.88 check --workspace --all-targets --exclude ulo-http-salvo --exclude
  ulo-graphql-async-graphql`, each print the same 17 warnings, all in `crates/ulo/src`, the set
  listed in `batch2a-cfgattr.md`.
- `cargo +1.98.1 clippy -p ulo-macro-lints --all-targets --no-deps --locked -- -D warnings` passes.
- `cargo test --workspace --no-fail-fast`, with the OpenSSL flags: 417 passed, 0 failed, 64
  ignored: batch 11's 413 plus the `ulo-rpc` unit test, two `post_deadline_body` tests and one
  `route_timeout` test.
- `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --lib` over `ulo-rpc`, `ulo-http`,
  `ulo-rpc-conformance` and `ulo-http-conformance`: no warning.
- The workflow parses as YAML, and `actionlint` (run from its image) reports only the SC2086 note
  on the `msrv` job named above. The print step's script was run under `bash -e` with the
  directory missing and with one file in it.
