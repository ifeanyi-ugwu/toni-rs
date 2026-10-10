# Divergences: runtime neutrality, stage d3: the WebSocket conformance suite, `ulo-ws-conformance`, run against the standalone server and the hand-off on the HTTP server's port

"The standalone WebSocket server and a WebSocket suite" (`RESPONSE.md`) asks for a suite in the
shape of the HTTP and RPC ones, through which an outside WebSocket server proves itself; stage d
built the split and left the suite. This stage builds it as `ulo-ws-conformance`: a `Host` trait
a server's crate implements in its `tests/`, 29 scenarios in seven groups, and
`ws_conformance_suite!` stamping one `#[test]` per scenario. The suite names no runtime and has no
tokio in its tree: a scenario runs inside the host's `block_on` on the app's runtime, and speaks to
the server over a `futures-io` stream the host opens. `ulo-ws-hyper`'s standalone server is the
reference and passes every scenario; the upgrade hand-off on the HTTP server's port, served by
`ulo-http-hyper`, runs the same 29 and passes them, one asserting the status alone (F368).
Thirteen moved `ulo-ws-hyper` tests that a scenario covers entirely are removed.

Files changed: the workspace `Cargo.toml` (member) and `Cargo.lock`; the new crate
`crates/ulo-ws-conformance` (`Cargo.toml`, `src/{lib.rs, app.rs, client.rs, failures.rs}`,
`src/cases/{mod.rs, handshake.rs, messages.rs, close_codes.rs, keep_alive.rs, hooks.rs, limits.rs,
stream_end.rs}`); `crates/ulo-ws-hyper/{Cargo.toml, tests/conformance.rs (new),
tests/handshake.rs, tests/limits.rs, tests/stream_end.rs}`; `crates/ulo-ws/{Cargo.toml,
tests/conformance.rs (new)}`; F368 filed in the workspace's `FRAMEWORK_GAPS.md`. No source file of
`ulo-ws` or `ulo-ws-hyper` changes. The tree is `26a46414` plus this stage; during the build the
transports DESIGN fold committed `f2349872`, which touches DESIGN.md alone.

## The signatures

```rust
// ulo_ws_conformance, new crate, `publish = false`
pub trait Host: Sized + 'static {
    type Runtime: ulo::Runtime;
    type Stream: futures_io::AsyncRead + futures_io::AsyncWrite + Unpin + 'static;
    const PORT: ulo_ws::Port;
    fn runtime() -> Self::Runtime;
    fn block_on<F: Future>(fut: F) -> F::Output;
    fn bind(app: App<Connected>) -> App<Connected>;
    fn connect(addresses: &[BoundAddr]) -> impl Future<Output = io::Result<Self::Stream>>;
}

pub const PATIENCE: Duration;          // 5 s, every wait a scenario makes
pub const DRAIN: Duration;             // 4 s, the suite app's drain window
pub const PARALLEL_VAR: &str;          // "ULO_CONFORMANCE_PARALLEL"
pub fn report(error: &(dyn Error + 'static)) -> String;
pub fn failures_dir() -> PathBuf;
macro_rules! startup_failed { .. }
macro_rules! ws_conformance_suite {
    ($host:ty) => { .. };
    ($host:ty; not_applicable { $($scenario:ident : $why:literal),* }) => { .. };
}
pub mod cases {                        // one `pub async fn scenario<H: Host>()` each
    pub mod handshake; pub mod messages; pub mod close_codes; pub mod keep_alive;
    pub mod hooks; pub mod limits; pub mod stream_end;
}
```

The `__private` module is doc-hidden: `startup_failed` and `Slots`. A host is two calls and two
values: `bind` binds the server under test to the suite's connected app on a port the OS chooses,
and `connect` opens one client connection to the addresses `listen()` reported; `runtime` and
`block_on` are the runtime suite's `Harness`, and `PORT` names the gateways the server serves. The
suite builds the app, runs `listen()`, serves it on the app's runtime, and closes it.

Dependencies of `ulo-ws-conformance`: `ulo`, `ulo-transport`, `ulo-ws`, `async-tungstenite` with
`handshake` alone (the key and the accept value), `event-listener`, `futures-io`, `futures-util`
with `io`, and `serde_json`. Both host crates gain it as a dev-dependency. `Cargo.lock` gains the
package and no other.

## Decisions

### 1. What the trait asks of a server

- **What a WebSocket server does, observably:** it binds into the app as a `Server`, accepts
  connections, answers each upgrade request, and drives each upgraded connection. The first is
  `bind`; the accepting and answering are what the scenarios observe through `connect`. Everything
  else (the app, its gateways, `listen()`, serving, the drain, the client) is the suite's, so the
  drain each scenario observes is the core's, run by the suite's `AppHandle::close`.
- **`connect` answers a `futures-io` stream:** a smol server answers `async-net`'s `TcpStream`,
  which implements those traits; a tokio one wraps its `TcpStream`, as both hosts here do with
  `Upgraded::from_tokio`. A TLS server's host performs the client side of its handshake in
  `connect`. The client the suite runs over it is `async-tungstenite` with no runtime feature.
- **No hyper, tokio or socket type** appears in the trait or in any public signature of the crate.

### 2. The runtime comes from the host

`runtime()` and `block_on()` are `ulo-runtime-conformance`'s `Harness`: each `#[test]` calls
`block_on` once, and the scenario builds the app inside it on `runtime()`. Every wait is bounded by
the app's `Timer`. `ULO_CONFORMANCE_PARALLEL` is read as the HTTP suite reads it, but a slot is
taken by blocking the test thread on a `std` condition variable before `block_on`, since the
other suites' tokio `Semaphore` would put tokio in the tree. `cargo tree -p ulo-ws-conformance -e
normal -i tokio` finds no tokio package. For stage e: both hosts run on tokio; no smol host exists
to run the suite on another runtime.

### 3. The gateways are declared once per port

`Port` is a word in the gateway attribute (`port = own`), so one gateway type cannot serve both
servers. `app.rs` writes the nine gateways once inside a `macro_rules!` that takes the word, and
stamps them into the modules `own` and `http`; `SuiteModule` imports `WsModule::for_root()` and
registers the set `Host::PORT` names. Every limit a scenario reads is set on the gateway's own
attribute, so a server's `GatewayDefaults` and `WsModule`'s change nothing the suite asserts.

| Path | Attribute | Used by |
| --- | --- | --- |
| `/echo` | plain | round trip, envelopes, control frame, stream, drain, `on_disconnect` |
| `/versioned` | `subprotocols = ["v2.chat", "v1.chat"]` | subprotocol choice |
| `/strict` | `refuse = handshake`, a connect guard on `x-token` | 401 and 403 |
| `/refused` | a connect guard, an `OnConnect` refusing by `x-refuse` | the close codes, `OnConnect` refusing |
| `/small` | `message_limit = 128`, `max_connections = 1` | 1009, 1013 |
| `/serial` | `max_inflight = 1` | `max_inflight` |
| `/outbound` | `max_outbound = 1` | slow consumer, stream under `max_outbound` |
| `/heartbeat` | `ping_interval` 100 ms, `pong_timeout` 150 ms | keep-alive |
| `/streams` | `on_stream_end` reporters, `AnswerOne` interceptor | stream end |

### 4. Stamped as plain `#[test]`s

The macro stamps `#[test] fn name() { H::block_on(cases::group::scenario::<H>()) }`, as the
runtime suite does, so an invoking crate needs no `#[tokio::test]`. `not_applicable`,
`startup_failed!`, the failures directory (`target/conformance-failures`, or
`$ULO_CONFORMANCE_FAILURES`) and a declared name that is no scenario failing to compile are the
other two suites', and were each run once (the section "The tests").

### 5. What both ports assert alike, and where they differ

- **A path without a gateway:** the status, 404, alone. On the HTTP server's port that path is the
  HTTP application's, which answers its own 404 document.
- **No request lacks an `Upgrade` header:** without one, a request on the HTTP server's port is an
  HTTP request and never reaches the hand-off. The 400 scenarios keep the header and spoil
  something else: HTTP/1.0, `Upgrade: h2c`, no `Connection: Upgrade`, no `Sec-WebSocket-Key`.
- **The drain's 503:** on a standalone server the handshake's `text/plain` refusal; on the HTTP
  server's port the status alone, the HTTP server answering every request at its drain with its own
  problem document before any upgrade handler sees it (F368, S3).
- **Every other scenario** asserts the same on both, the 405's `Allow`, the 426's
  `Sec-WebSocket-Version` and each refusal's plain-text reason included.
- **An unknown event** is asserted as kind `unimplemented`: transports DESIGN §4.2 and §12 and
  `message` in `crates/ulo-ws/src/connection.rs` give `Unimplemented`, its source `NoHandler`.
  The brief named `NotFound`, which the thirty-eighth response's S7 rules wrong. See S7.

### 6. Absences read after a positive signal

- **A discarded stream reports nothing:** each `/streams` handler's `on_stream_end` callback owns a
  `Reporter` whose `Drop` records `Ended`, so the scenario waits for `Ended`, which the execution's
  end produces whether the callback ran or not, then reads the record.
- **`max_inflight` holds a message unread:** after `open` is sent behind a held `hold`, a round trip
  on a second `/serial` connection completes before the gate opens; the gate's log then shows
  `open` ran after `hold`, as the moved test asserts.
- **A refused connection never reaches `on_disconnect`:** read after the app's close, whose drain
  waits for every connection's task.
- **A control frame gets nothing else:** after a Ping, an unsolicited Pong and a message, the frames
  before the message's answer are exactly the Ping's Pong.

### 7. The drain's 503 needs a connection accepted before the drain

A server stops accepting at its drain, and `ulo-hyper-serve` drops its listener, which resets the
connections still queued unaccepted. hyper's graceful shutdown keeps a connection that has not
finished its first request (`KA::Busy` from the start, `hyper` 1.11.1 `proto/h1/conn.rs:57`). So the
scenario writes the request line on a connection, opens the idle `/echo` connection after it and
waits for its 101, starts the close, waits for that connection's 1001, then writes the rest of the
head. One listener accepts in arrival order, so the 101 shows the pending connection was accepted.
The first version opened the pending connection after the idle one; it failed with a broken pipe
in 7 of the 17 break runs whose break did not touch it. Reordered, it passed 100 of 100 isolated
runs on each host. See S5.

### 8. The slow consumer on any executor

The moved test pins that the Close is the first frame, which holds on a current-thread runtime
where a handler's sends all queue before the loop writes. The scenario cannot choose the host's
executor, so the burst is 1,000 frames without a yield and the scenario reads past any data frames
to the Close, `(1008, "slow consumer")`, requiring fewer than 1,000 to have arrived and the
departure to be `ServerClose { code: 1008 }`. The moved test stays. See S6.

### 9. The same-port host fits the trait

`HttpPort` (`crates/ulo-ws/tests/conformance.rs`) declares `PORT = Port::Http` and binds
`ulo_http_hyper::Server`; nothing else differs from the reference host. It lives in `ulo-ws`, whose
hand-off it tests, as `ulo-http-conformance` is a dev-dependency of the crate it depends on.

### 10. CI

The `test` job runs `cargo test --workspace --locked`, which builds and runs both stamped suites
and the crate's unit test. Nothing is added to CI.

## The scenarios

**Handshake (8)**

- `handshake_switches_with_the_accept_key`: 101 with RFC 6455 §1.3's `Sec-WebSocket-Accept`,
  `Upgrade` and `Connection`, no subprotocol, then a round trip on the socket.
- `handshake_refuses_a_path_without_a_gateway`: 404.
- `handshake_refuses_a_method_other_than_get`: 405, `Allow: GET`, a plain-text reason.
- `handshake_refuses_a_malformed_upgrade`: 400 for HTTP/1.0, `Upgrade: h2c`, no `Connection:
  Upgrade` and no key, each with a plain-text reason.
- `handshake_refuses_another_version`: 426 with `Sec-WebSocket-Version: 13`.
- `handshake_refuses_during_the_drain`: 503 to a head finished after the drain's 1001 (decision 7).
- `subprotocol_choice`: five offers, the 101 echoing the gateway's first listed that the client
  offered, offers over two header lines read as one list, none echoed when none matches, each
  matching what the connection's `UpgradeHead` reports.
- `refuse_handshake_answers_401_or_403`: 403 for the connect guard, 401 with `WWW-Authenticate:
  Bearer` and `OnConnect`'s reason, then an admitted round trip.

**Messages (6)**

- `event_round_trip`: `{"id","data"}` back, a string `id` echoed as sent.
- `unknown_event_is_unimplemented`: the `error` envelope of kind `unimplemented`, with the `id` and
  without one; the connection then answers.
- `frame_naming_no_event_is_bad_request`: `bad_request` for an object with no `event`, carrying its
  `id`, and for text that is not JSON, without one; the connection then answers.
- `control_frame_is_answered_with_nothing`: the Ping's Pong and nothing else before the next
  message's answer (decision 6).
- `message_limit_closes_with_1009`: two fragments under 128 bytes, together over, close with 1009;
  `on_disconnect` receives `ProtocolError`.
- `reply_stream_is_written_whole`: five items in order, then `complete`, then a round trip.

**Close codes (5)**

- `connect_guard_refusal_closes_with_1008`.
- `rate_limited_connect_closes_with_1013`: `TooManyRequests`, with its reason.
- `faulted_connect_closes_with_1011`: `Internal`, and a panic in `OnConnect`.
- `connect_refusal_closes_with_its_own_code`: `ConnectRefused::code(4406, ..)`, code and reason,
  this branch's form of `WsError::Refused`.
- `drain_closes_with_1001`: an idle connection gets `(1001, "server shutting down")`, and
  `on_disconnect` exactly `Drain`.

**Keep-alive (2)**

- `ping_interval_sends_pings`: three Pings, each answered, over longer than `pong_timeout`; the
  connection answers and has not ended.
- `pong_timeout_ends_the_connection_as_lost`: one Ping left unanswered; `on_disconnect` receives
  `Lost`, and the connection is dropped without a Close frame.

**Hooks (2)**

- `on_connect_refusal_never_reaches_on_disconnect`: a refused connection and an admitted one;
  after the app's close the record holds the admitted one's `ClientClose` alone.
- `on_disconnect_fires_once`: `ClientClose { 4001, "bye" }`, exactly once by the app's close.

**Limits (4)**

- `max_connections_closes_the_next_with_1013`: the second connection gets `(1013, "too many
  connections")`, and the first still answers.
- `max_inflight_stops_reading`: decision 6's ordering.
- `max_outbound_closes_a_slow_consumer`: decision 8.
- `max_outbound_holds_a_reply_stream`: 50 items through a queue of one, `complete`, a round trip,
  and the end `ClientClose { 1000 }`, no server close.

**Stream end (2)**

- `written_stream_reports_completed`: the three frames, then the record `[Outcome(Completed),
  Ended]`.
- `discarded_stream_reports_nothing`: the interceptor's frame, then the record `[Ended]`.

## The tests

### The hosts

| Host | File | `PORT` | Server | Runs |
| --- | --- | --- | --- | --- |
| `Standalone` | `crates/ulo-ws-hyper/tests/conformance.rs` | `Own` | `ulo_ws_hyper::Server::new("127.0.0.1:0")` | 29 passed, three runs, 0.32 s each |
| `HttpPort` | `crates/ulo-ws/tests/conformance.rs` | `Http` | `ulo_http_hyper::Server::new("127.0.0.1:0")` | 29 passed, three runs, 0.31–0.32 s each |

Both build a multi-thread tokio runtime per scenario in `block_on` and connect over
`tokio::net::TcpStream` wrapped in `Upgraded::from_tokio`. One unit test in the suite's
`__private`, `a_bound_of_one_admits_one_holder_at_a_time`, holds three threads to one slot through
the real `hold_under` path.

The macro's own paths, each run once on `Standalone`: `ULO_CONFORMANCE_PARALLEL=2`, 29 passed;
`ULO_CONFORMANCE_PARALLEL=zero`, the scenario failed with "`ULO_CONFORMANCE_PARALLEL` is `zero`, not
a positive integer" and the message written to a file under `$ULO_CONFORMANCE_FAILURES`;
`not_applicable { subprotocol_choice: "probe" }`, 28 passed and 1 ignored; a declared
`no_such_scenario`, E0425 at the declaration. The host file was restored from a copy and checked by
hash.

### Breaks

Each break rewrote one or more spans through `runtime-d3/brk.py`, which asserts each span occurs
once, runs the targets, writes every file back byte for byte, and compares a hash over `git diff`
of `crates`, `Cargo.toml` and `Cargo.lock` and every file the stage added; every restore reported
`ok` with the hash unchanged. Specs are in `runtime-d3/specs/` and `specs2/`, full output in
`runtime-d3/broken/`. Against `Standalone` unless the row says `HttpPort`.

| Group | Break | Failed |
| --- | --- | --- |
| handshake | 426 answered as 400 | `handshake_refuses_another_version`; on `HttpPort` too |
| handshake | the subprotocol chosen in the client's order | `subprotocol_choice` |
| handshake | no 503 at the drain | `handshake_refuses_during_the_drain`, before and after decision 7's reorder |
| handshake | `ulo-ws-hyper` writing a refusal without its body | the four refusal scenarios and the 401/403 one |
| handshake | the accept key derived from the wrong input | 25 of 29, at the 101's `Sec-WebSocket-Accept` |
| handshake | `Unauthorized` answered 403 | `refuse_handshake_answers_401_or_403` |
| messages | an unknown event as `NotFound` | `unknown_event_is_unimplemented`; on `HttpPort` too |
| messages | an unparseable frame as `Internal` | `frame_naming_no_event_is_bad_request` |
| messages | a Ping answered with a data frame too | `control_frame_is_answered_with_nothing` |
| messages | no `message_limit` | `message_limit_closes_with_1009` |
| messages | a stream ending without `complete` | `reply_stream_is_written_whole` and the two scenarios reading `complete` |
| close codes | `TooManyRequests` closing with 1008 | `rate_limited_connect_closes_with_1013` |
| close codes | the drain closing with 1000 | `drain_closes_with_1001` and the 503 scenario, which waits for the 1001; on `HttpPort` too |
| close codes | `ulo-ws-hyper`'s drain leaving the table out | the same two |
| keep-alive | no Ping sent | both keep-alive scenarios; on `HttpPort` too |
| keep-alive | a missed Pong ignored | `pong_timeout_ends_the_connection_as_lost` |
| hooks | `on_disconnect` run twice | both hooks scenarios and the five others reading the record exactly; on `HttpPort` too |
| limits | no `max_connections` | `max_connections_closes_the_next_with_1013` |
| limits | no `max_inflight` | `max_inflight_stops_reading`; on `HttpPort` too |
| limits | overflow dropping the oldest instead of closing | `max_outbound_closes_a_slow_consumer` |
| stream end | the written stream reported `CutOff` first | `written_stream_reports_completed` |
| stream end | a single reply reporting `Completed` | `discarded_stream_reports_nothing`; on `HttpPort` too |
| hand-off | `Handoff::upgrade` dropping the `Switch` unserved | 25 of 29 on `HttpPort` |
| tree | `tokio` added to the suite's dependencies | `cargo tree -p ulo-ws-conformance -e normal -i tokio` printed it; manifest and lock restored from copies, checked by hash |
| restored | every target | both suites 29 passed, three runs each |

Under the first break round the 503 scenario also failed in 7 runs whose break did not touch it,
each with a broken pipe, which decision 7's reorder ended. The second round, after it, shows no
such failure.

### Overlaps with the moved `ulo-ws-hyper` tests

A test is removed only where a scenario asserts all it asserts on the same server.

| Test | Scenario | Done |
| --- | --- | --- |
| `handshake.rs` `the_101_echoes_the_first_listed_subprotocol_the_client_offered` | `subprotocol_choice` (its three offers, and the `UpgradeHead`) | removed |
| `handshake.rs` `offers_spread_over_two_header_lines_are_read_as_one_list` | `subprotocol_choice` | removed |
| `handshake.rs` `a_gateway_that_does_not_refuse_proceeds_without_a_subprotocol` | `subprotocol_choice`, "none the gateway lists" | removed, with its `Lenient` gateway |
| `handshake.rs` `refuse_handshake_answers_a_guard_refusal_403_before_the_upgrade` | `refuse_handshake_answers_401_or_403` | removed |
| `handshake.rs` `refuse_handshake_answers_an_unauthorized_refusal_401_with_a_challenge` | the same | removed |
| `limits.rs` `the_message_limit_counts_a_message_after_reassembly` | `message_limit_closes_with_1009` | removed |
| `limits.rs` `an_integer_max_connections_admits_that_many_and_closes_the_next_with_1013` | `max_connections_closes_the_next_with_1013`, also an integer literal | removed |
| `limits.rs` `max_inflight_of_one_reads_nothing_more_until_the_message_in_flight_is_answered` | `max_inflight_stops_reading` | removed, with its `Serial` gateway |
| `limits.rs` `a_streamed_answer_longer_than_max_outbound_waits_for_room_and_is_written_whole` | `max_outbound_holds_a_reply_stream` | removed, with `Strict::count` |
| `limits.rs` `a_pong_that_misses_pong_timeout_ends_the_connection_as_lost` | `pong_timeout_ends_the_connection_as_lost` | removed |
| `limits.rs` `a_client_answering_each_ping_outlives_the_pong_timeout` | `ping_interval_sends_pings` | removed, with its `Heartbeat` gateway |
| `stream_end.rs` `a_written_stream_reports_completed` | `written_stream_reports_completed` | removed |
| `stream_end.rs` `a_stream_an_interceptor_discards_reports_nothing` | `discarded_stream_reports_nothing` | removed, with its handler and `AnswerOne` |
| `attributes.rs` `stream_reply_writes_each_item_then_completes` | `reply_stream_is_written_whole` | kept: the file drives one handler per reply kind on one gateway, and the removal would leave `count` undriven |
| `attributes.rs` `connect_guard_refusal_closes_the_connection_with_policy_violation` | `connect_guard_refusal_closes_with_1008` | kept: its guard is named by type, the scenario's written by value |
| `attributes.rs` `on_connect_refusal_closes_with_its_own_code` | `connect_refusal_closes_with_its_own_code` | kept: its gateway also builds a session through `session_with` |
| `handshake.rs` `no_offered_subprotocol_gets_a_101_without_one_and_the_gateway_refuses` | `subprotocol_choice` | kept: the gateway's own 4406 refusal is not in the scenario |
| `hooks.rs` `a_close_the_client_sends_is_client_close_with_its_code_and_reason` | `on_disconnect_fires_once` | kept: it reads the session in `on_disconnect` |
| `hooks.rs` `the_drain_is_drain_and_on_disconnect_still_runs_and_reads_the_session` | `drain_closes_with_1001` | kept: the session again |
| `hooks.rs` `a_connection_the_connect_phase_refused_never_reaches_on_disconnect` | `on_connect_refusal_never_reaches_on_disconnect` | kept: it refuses through a guard as well as `OnConnect` |
| `limits.rs` `a_message_over_the_limit_closes_with_1009` | `message_limit_closes_with_1009` | kept: one frame over the limit, where the scenario sends two |
| `limits.rs` `an_outbound_queue_over_its_limit_closes_with_slow_consumer` | `max_outbound_closes_a_slow_consumer` | kept: it pins the Close as the first frame (decision 8) |
| `ulo-ws` `handoff.rs` `the_drain_closes_a_same_port_connection_with_going_away` | `drain_closes_with_1001` on `HttpPort` | kept: outside the moved files |

The three edited files' module docs now state what they pin beyond the suite. `ulo-ws-hyper` runs
68 tests where it ran 52: 13 removed, 29 added.

## Left for the transports DESIGN fold

Line numbers are those read on `f2349872`.

- §1's crate table, after `fw-runtime-conformance` (line 45): a row for `fw-ws-conformance`: the
  WebSocket conformance suite, the `Host` trait (`Runtime`, `Stream` on `futures-io`, `PORT`,
  `runtime`, `block_on`, `bind`, `connect`), 29 scenarios in seven groups, one macro stamping each
  per host, `fw-ws-hyper`'s standalone server the reference and the hand-off on the HTTP server's
  port run too; no runtime of its own.
- §4.1, X32 (line 753): "A WebSocket conformance suite in the shape of the HTTP and RPC suites ...
  is not built" is built as above; the gateways the suite declares once per `Port`, and the
  scenario list with the two port-dependent assertions (decision 5).
- §4.1 or §3.5 (line 484): the drain's 503 on the HTTP server's port is the HTTP server's problem
  document; the table's 503 is a standalone server's (F368).
- §5.2's `FW_CONFORMANCE_PARALLEL` (line 895): the WebSocket suite applies the variable alone, as
  the HTTP suite does, its slot taken by blocking the test thread before the scenario's runtime
  starts.
- §8's runtime table (line 1227): `fw-ws-conformance` joins the no-tokio row beside
  `fw-runtime-conformance`; line 1235 drops "and a WebSocket conformance suite (§4.1)".
- Decisions 33 (line 1436) and 42 (line 1445) name "either suite" and §3.8, §5.2: three suites with
  stamped scenarios, and §4.1.

## Needs sign-off

### S1. The host is `bind` and `connect`, with the runtime suite's `Harness` beside them

Decision 1: the suite owns the app, its drain and the client, and a host supplies the server and a
byte stream. The alternatives are a stateful `start() -> Self` and `stop()` as the RPC suite's
`Broker` has, which no current host needs, or the host running the client, which would put each
server's own client between the suite and the wire.

### S2. The gateways are declared twice by a macro

Decision 3: `port = own` takes a word, so the set is stamped once per port. The alternative is an
attribute value read from an expression or a type parameter, an attribute change for a test's sake.

### S3. Two assertions depend on the port

Decision 5: a 404 is asserted by status on both, and the drain's 503 by status alone on the HTTP
server's port. The alternatives are F368's: the HTTP server handing an upgrade request to the
hand-off before its drain check, after which the 503 is asserted alike, or the suite asserting the
HTTP server's problem document there.

### S4. Thirteen moved tests removed

The section "Overlaps": each is asserted in full by a scenario on the same server. The
alternative is keeping all 52.

### S5. The 503 scenario relies on accepting in arrival order

Decision 7: the pending connection is shown accepted by the 101 of the connection opened after it.
A server accepting on several loops per address, or out of order, could fail the scenario without a
fault; such a server would declare it `not_applicable` with its reason. The alternative is
dropping the scenario; no other test reaches the handshake's 503.

### S6. The slow-consumer scenario lets data frames precede the Close

Decision 8: the moved test keeps the stricter form on a current-thread runtime. The alternative is
the suite requiring a host's executor to be current-thread.

### S7. An unknown event is `unimplemented`

Decision 5: the brief names `NotFound`; the design and the code say `unimplemented`, and the suite
follows them.

*Settled by the thirty-eighth response:* `unimplemented` is the intended answer, a gateway knowing
for certain that no handler exists for the event; `NotFound` would describe a missing resource.
The brief's `NotFound` was wrong.

## Verification

Full, unfiltered output of every run is in the session scratchpad, `runtime-d3/`: `first/`,
`suites/`, `broken/`, `specs/`, `specs2/`, `drain-loop/`, `tree/`, `na/` and `verify/`.

- **Tests:** `cargo test --workspace --no-fail-fast` with the OpenSSL flags: 582 passed, 0 failed,
  70 ignored across 144 test binaries, counted from the `test result:` lines: 536 before, 13 removed,
  29 per host and the suite's unit test; the two ignored added are the suite's `ignore` doc
  examples. Each suite three runs, 29 passed each (`suites/`).
- **The drain scenario alone:** 100 of 100 on each host from copies of the built binaries
  (`drain-loop/`); the loop's checker counted a run of zero tests as failed, run first against a
  name that matches none.
- **The tree:** `cargo tree -p ulo-ws-conformance -e normal -i tokio`, with and without
  `--target all`, prints nothing and exits 101, "package ID specification `tokio` did not match any
  packages": tokio is in none of the crate's graph. Planted as a dependency, it printed tokio
  (`tree/planted.txt`); `ulo-ws-hyper`, tokio-bound, prints it.
- `cargo check --workspace --all-targets --all-features` with
  `CFLAGS=-I/opt/homebrew/opt/openssl/include LDFLAGS=-L/opt/homebrew/opt/openssl/lib`, and with
  defaults: exit 0, the 17 known warnings in `crates/ulo/src` and no other.
- `cargo +1.88 check --workspace --all-targets --exclude ulo-http-salvo --exclude
  ulo-graphql-async-graphql`: exit 0, the same 17.
- `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --lib` for `ulo-ws-conformance`, `ulo-ws-hyper`
  and `ulo-ws`: each exits 0.
- `cargo +1.98.1 clippy -p ulo-ws-conformance -p ulo-ws-hyper --all-targets --no-deps --locked`:
  exit 0, no warning in either crate, the 17 rustc warnings of `ulo` aside. A `len_zero` violation
  planted in the suite was reported, then removed and checked by hash.
- **CI:** the `test` job's `cargo test --workspace --locked` runs both suites; nothing added.
- **Containers:** none started; the user's four untouched. No permission check refused an action.
