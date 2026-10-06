# Divergences: race 2b, tests batch 2, the RPC conformance suite over TCP and UDP

`ulo-rpc-conformance` had never run against a link. This batch wires it into the two links that
need no broker, TCP and UDP, through a `Broker` implementation in each crate's `tests/` and the
suite's `conformance_suite!`. All twenty scenarios passed on both links on the first run. No source
outside the two test files and the two manifests changed.

Files added: `crates/ulo-rpc-tcp/tests/conformance.rs`, `crates/ulo-rpc-udp/tests/conformance.rs`.
Files changed: `crates/ulo-rpc-tcp/Cargo.toml` and `crates/ulo-rpc-udp/Cargo.toml` (dev-dependencies
`ulo-rpc-conformance` and tokio's `rt-multi-thread`, which `#[tokio::test(flavor = "multi_thread")]`
names; `macros` comes from the normal dependency), `Cargo.lock`.

## The signatures

No public signature changed.

## Fixes

None. No scenario failed on either link, so no side was found wrong against transports DESIGN §5,
`RESPONSE.md` or `DIVERGENCES_2B.md`.

What each link's capabilities make the suite assert:

| Scenario | TCP (`ALL_SHAPES`, JSON, `max_frame` 4 MiB, `Addressed`, `miss_signal`) | UDP (`UNARY_ONLY`, JSON, `max_frame` 65,507, `Addressed`, `miss_signal`) |
| --- | --- | --- |
| `server_stream_in_order`, `client_stream`, `bidi_stream`, `cancel_mid_stream` | the stream runs; cancel records `ClientCancelled` | `StartupError::Configure` for a server mounting `StreamController` (race2b-B entry 15) |
| `binary_payload` | the client's `binary_unsupported`, then the startup refusal of `BinaryController` | the same |
| `oversized_payload` | `payload_too_large` one byte over 4 MiB | `payload_too_large` one byte over 65,507 |
| `unhandled_pattern` | `Unavailable` with `pattern_unhandled` or `no_destination` | the same |
| `drain` | the new call `Unavailable` (an `Addressed` link may not answer `Timeout`), the held call finishing, the stream ending cleanly | the new call `Unavailable`, the held call finishing; no stream |
| `two_instances` | one instance receives all ten events | the same |
| `recovery_after_disrupt` | `disrupt` does nothing (below) | `disrupt` does nothing |

## Decisions

### 1. A fresh port per scenario, released before the server binds it

- **Written:** `Broker::start` binds a loopback socket of the link's kind on port 0, reads the
  port and drops the socket; `link()` answers `Tcp::new(addr)` / `Udp::new(addr)` on that address
  for both halves.
- **Why:** `link()` builds the server's and the client's link alike and cannot tell which it is
  building, so both must name one concrete address. Port 0 would leave the client nothing to
  connect to, and neither link takes a pre-bound socket outside the activation protocol.
- **Cost:** another process may take the port between the drop and the server's bind. The server
  then fails `bind` and the scenario panics with "the conformance server did not start". It did
  not happen in the runs below.

### 2. `disrupt` is a no-op on both links

- **Written:** an empty `disrupt`. On TCP the broker holds none of the client's connections; on
  UDP there is no connection.
- **Why:** severing the client's TCP connection from the broker side needs a proxy between client
  and server, and `link()` gives both halves one address, so the client cannot be pointed at a
  proxy while the server listens elsewhere. §5.2 states the recovery scenario for the brokers.

## Not covered

- **The client reconnecting after a lost TCP connection.** `RpcClient` documents that a lost reply
  lane fails the calls waiting on it `Unavailable` and that the next call connects again. On TCP
  `recovery_after_disrupt` passes without exercising that path (decision 2). Covering it needs
  either a `Broker` method that hands out the client's link separately from the server's, or a
  TCP-specific test outside the suite.
- **The suite against a known violation.** Every scenario passed on its first run on both links.
  Running it against a link or a `ulo-rpc` with a check removed (the shape refusal in
  `Server::prepare`, the TCP link's id mapping for `cancel`) was attempted and not permitted in this
  session, so nothing here shows that a scenario fails when its contract breaks.

## Verification

- `cargo test -p ulo-rpc-tcp --test conformance` and `cargo test -p ulo-rpc-udp --test
  conformance` on stable, three times each after the first run: 20 passed, 0 failed, every run.
  The suite's own time was 1.31 to 1.32 s per run on each link. The first build of the TCP test
  took 42 s (cold, behind the target-dir lock); the UDP one 5 s.
- No flake was seen in the four runs per link. The one flake source identified is decision 1's
  port race.
- `cargo check --workspace --all-targets` on stable: the 17 warnings it prints are all in
  `crates/ulo/src` (the `BindingRecord` `private_interfaces` set and dead code in `binding/`,
  `dependency/`, `graph/`, `lifecycle/`, `module/`, `redact.rs`, `timer.rs`), the same set listed in
  `batch2a-cfgattr.md`; none is in a file this batch adds. The run was served from cache, so it was
  not confirmed to have rebuilt the new test targets.
- Not run, refused in this session: `cargo +1.88 check --workspace --all-targets --exclude
  ulo-http-salvo --exclude ulo-graphql-async-graphql`, and `cargo test --workspace`.
