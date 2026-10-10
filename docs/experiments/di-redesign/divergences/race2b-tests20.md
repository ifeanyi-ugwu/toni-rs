# Divergences: race 2b, tests batch 20, paired connection hooks on every unserved path (S8) with F371 found and fixed, `RpcClient::timeout` refusing a zero as `ZeroTimeout` (S3), one `Watch` in `ulo-transport` (S4), the standalone server's report order pinned (S12), F368 settled as documentation, the drain scenario on observable acceptance (S5), and the unknown event's kind corrected in the record (S7)

The thirty-seventh and thirty-eighth responses, signed off 2026-10-10, direct the follow-ups of
runtime stage d (S3, S4, S8, S12 of `runtime-d.md`) and stage d3 (F368, S5, S7 of
`runtime-d3.md`). Under `refuse = handshake` the connection phase runs `OnConnect` before a
`Switch` exists, so every path that ends an admitted connection unserved now runs `on_disconnect`:
a dropped `Switch` and a failed upgrade with `Lost`, a connection over `max_connections` with
`ServerClose { code: 1013 }`. Checking S8 on a real socket found F371: hyper drops a request whose
client has gone, which cut `OnConnect` off mid-await and left the connection in its rooms. The
connection phase now runs as a task on the table's tracker. `RpcClient::timeout` answers
`Result<Self, ZeroTimeout>`, the error the module path records at `wire()` too. `Watch` lives once,
in `ulo-transport`'s `__private`. The standalone server's `prepare` order is pinned and documented.
F368 is documented in `ulo-ws`'s hand-off docs and the suite. The drain scenario shows its
connection accepted by a 404 answered on it; both hosts close that connection when the drain
begins, which they declare through a new `Host::CLOSES_IDLE_AT_DRAIN`, so the scenario's 503
branch runs on no current host (F372). The d3 log's `NotFound` wording is corrected.

Files changed: `Cargo.lock`; `crates/ulo-transport/{Cargo.toml, src/lib.rs, src/__private.rs,
src/watch.rs (new)}`; `crates/ulo-rpc/{Cargo.toml, src/lib.rs, src/client.rs,
src/client_module.rs, src/dispatch.rs, src/server.rs, tests/runtime.rs}`, `crates/ulo-rpc/src/watch.rs`
(deleted); `crates/ulo-rpc-tcp/tests/handle.rs`; `crates/ulo-ws/{src/lib.rs, src/connection.rs,
src/handoff.rs, src/module.rs, src/table.rs, tests/table.rs, tests/conformance.rs}`,
`crates/ulo-ws/src/watch.rs` (deleted); `crates/ulo-ws-hyper/{src/lib.rs, tests/conformance.rs,
tests/abandoned_handshake.rs (new), tests/prepare.rs (new)}`; `crates/ulo-ws-conformance/src/{lib.rs,
app.rs, client.rs, cases/handshake.rs}`; `docs/experiments/di-redesign/divergences/runtime-d3.md`
(S7); in the workspace's `FRAMEWORK_GAPS.md`, a note under F368 and F371, F372 and F373 filed. The
tree is `430101b8` plus this batch; the transports DESIGN fold committed `430101b8` during the
build and touches DESIGN.md alone.

## The signatures

```rust
// ulo_rpc, new
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZeroTimeout { /* private: the setter's name, the link's name */ }
impl ZeroTimeout {
    pub fn link(&self) -> &'static str;                 // `Link::NAME`
}
impl fmt::Display for ZeroTimeout {}
impl Error for ZeroTimeout {}

// ulo_rpc, changed
impl RpcClient {
    pub fn timeout(self, timeout: Bound) -> Result<Self, ZeroTimeout>; // was `-> Self`, panicking on zero
}

// ulo_transport::__private, doc-hidden, moved from `ulo-rpc` and `ulo-ws`
pub struct Watch<T> { /* private */ }
impl<T> Watch<T> {
    pub fn new(value: T) -> Self;
    pub fn modify(&self, change: impl FnOnce(&mut T));
    pub fn read<R>(&self, read: impl FnOnce(&T) -> R) -> R;
    pub fn changed(&self) -> EventListener;
    pub async fn wait_for(&self, ready: impl Fn(&T) -> bool);
}

// ulo_ws_conformance, new associated constant
pub trait Host {
    const CLOSES_IDLE_AT_DRAIN: bool = false;
    /* .. */
}
```

`ZeroTimeout`'s `Display` is the module path's sentence with the setter named:
"`RpcClient::timeout(Bound::After(Duration::ZERO))` on the {link} link would time out every call;
write `Bound::Unbounded` to turn the timeout off", and `RpcClientModule::timeout` in place of
`RpcClient::timeout` for a module. `RpcClient::new(link, rt).timeout(b)?` therefore fails on the
line that wrote the zero, and a `ZeroTimeout` converts to `BoxError`. No handler, module or
controller signature changes, and no macro's output changes.

What changes in meaning without a signature: dropping a `Switch` that carries an admitted
connection runs `on_disconnect` with `DisconnectReason::Lost`, then unregisters the connection, on a
task the table's drain and close reach; `Switch::serve` whose upgrade future fails does the same; a
connection over `max_connections` whose `OnConnect` ran in the handshake gets `on_disconnect` with
`ServerClose { code: 1013 }` after its 1013 close; and the connection phase under
`refuse = handshake` runs as a task on the table's tracker, the handshake awaiting its answer.

Private: in `ulo-ws`, `connection_phase` and `Admitted::end`; in `ulo-ws-conformance`,
`read_response`, `read_head`, `read_body` and `within_for`; in `ulo-rpc`, `ZeroTimeout::of_module`.

Dependencies: `ulo-transport` gains `event-listener`, runtime-free and in the lock already;
`ulo-rpc` loses it, `Watch` having been its only user. `Cargo.lock` moves that one edge.

## Decisions

### 1. S8: `OnConnect` runs before a `Switch` exists, so every unserved path ends the connection

- **The finding:** yes. At `430101b8`, `connection::handshake` under `Refuse::Handshake` awaited
  `connect` (`crates/ulo-ws/src/connection.rs:524`), which registers the connection with the hub
  (`:729`) and runs the connect guards and `OnConnect` through `ulo::dispatch` (`:736`); the
  `Switch` carrying the admitted connection was built after it (`:535`). Under `refuse = close`
  the phase runs in `run`, after the upgrade, and a `Switch` carries nothing to end.
- **One way an admitted connection ends:** `Admitted::end(accept, why)` runs `disconnect`, the
  `on_disconnect` terminal execution, then unregisters the connection from the hub, the order the
  served path always had, so `on_disconnect` still sees the connection's rooms. Every path calls
  it (`connection.rs`):

| Path | Before | Now |
| --- | --- | --- |
| served, the read loop ended | `disconnect`, then unregister | `end(why)` |
| `Switch` dropped unserved | unregister, synchronously in `Drop` | `end(Lost)` on a task the tracker counts, spawned from `Drop` |
| `Switch::serve`'s upgrade future fails | unregister | `end(Lost)` |
| over `max_connections`, admitted in the handshake | unregister, then the 1013 close | the 1013 close, then `end(ServerClose { code: 1013 })` |

- **The fourth row** is the same rule on a path the response did not name: `OnConnect` ran and the
  server closed the connection. See S1.
- **The 101's write failing** is what the host does: hyper and the hand-off serve the `Switch` before
  writing the 101 and hand `serve` an upgrade future that fails when the write does (row three); a
  server writing the 101 first and dropping the `Switch` on failure takes row two.

### 2. F371: a client gone during `OnConnect`, found by a probe, fixed

- **The probe** (`probes/f371-hyper-client-gone.txt`, `probes/f371-handoff-client-gone.txt`): a
  `refuse = handshake` gateway whose `OnConnect` joined `lobby` and waited at a gate; the client
  sent the upgrade request, waited for `OnConnect` to start and dropped its socket; the gate opened
  300 ms later. On `ulo_ws_hyper::Server` and on the hand-off over `ulo_http_hyper::Server` alike,
  the `OnConnect` future was dropped at its await, `on_disconnect` never ran, and the connection was
  still in `lobby` after the app's close.
- **Why:** hyper detects the end of a busy connection's stream (`hyper-1.11.1/src/proto/h1/conn.rs:487-500`)
  and its dispatcher hands the error to the server, which returns it (`dispatch.rs:123-141`); the
  connection future ends and drops the response future the handshake ran in.
- **The fix:** `handshake` spawns the connection phase on the table's tracker and awaits its
  `Handshake` through a `futures-channel` oneshot. A phase nobody is waiting for runs to its end,
  and its `Switch`, dropped where the send fails or with the channel, ends the connection through
  row two. The drain counts a handshake in its connection phase, the reason the thirty-seventh
  response's S7 gives for spawning a connection inside the tracker; a phase the table's close aborts
  answers the handshake 503. The probe on the fix recorded `OnConnect` finishing, `on_disconnect
  Lost` and an empty room (`probes/f371-hyper-client-gone-fixed.txt`). See S2.

### 3. S3: `ZeroTimeout`, one type on both paths

- **What the module path did:** `RpcClientModule::register` recorded a formatted `String` through
  `m.try_value`, so `wire()` failed with `WiringError::ValueFailed` carrying text and no type
  (`crates/ulo-rpc/src/client_module.rs`). No error type existed.
- **Now:** `ZeroTimeout` is defined once in `client.rs`, exported from `ulo_rpc`, and recorded by
  the module as the value failure's error, so `error.downcast_ref::<ZeroTimeout>()` finds it in the
  `Redacted` the wiring error carries. `RpcClient::timeout` answers `Err(ZeroTimeout)` for
  `Bound::After(Duration::ZERO)` and drops the handle with it; `Bound::Default`, `After(d)` and
  `Unbounded` answer `Ok`, with the meanings they had. The `# Panics` section is gone, and the
  doc example ends in `?`.
- **Callers changed:** `crates/ulo-rpc/tests/runtime.rs` and `crates/ulo-rpc-tcp/tests/handle.rs`
  take the `Ok` with `expect`. See S4.

### 4. S4: one `Watch`, in `ulo-transport`'s `__private`

- **Where:** `crates/ulo-transport/src/watch.rs`, re-exported as `ulo_transport::__private::Watch`;
  `__private`'s module doc now names the primitives the transport crates share beside the codegen's
  support. The type is `ulo-ws`'s copy, which was `ulo-rpc`'s plus `changed()`; `ulo-rpc` reads
  `changed()` nowhere.
- **The ordering kept:** `wait_for` registers its listener, then reads the value, then awaits, as
  both copies did; the struct's doc states why an `Event` needs it.
- **No runtime:** `cargo tree -p ulo-transport -e normal -i tokio` finds no tokio, with and without
  `--all-features --target all`.
- **Rustdoc:** the item is doc-hidden, so `RUSTDOCFLAGS="-D warnings"` does not check its links; a
  broken link planted in it passed, and one planted in the crate's visible doc failed
  (`broken/doc-planted*.txt`).

### 5. S12: the order stays, pinned and documented

The standalone server's `prepare` pushes its two timeout refusals, then calls
`GatewayTable::own_port`, whose own refusals follow (the server's gateway defaults under its name,
its gateways, the absence of any `port = own` gateway), then resolves endpoints and loads TLS
(`crates/ulo-ws-hyper/src/lib.rs`, `prepare`). That sequence is deterministic already: `Failures`
keeps the order pushed and `build_table` walks the mounted handlers as listed. Nothing in the code
changes; `Server`'s doc states the order, and `tests/prepare.rs` pins it.

### 6. F368, option (a): documented in two places

- **The hand-off's docs:** `crates/ulo-ws/src/handoff.rs`'s module doc gains the rule, "On the HTTP
  port, the HTTP server's drain and load shedding answer first", with what follows from it: an
  upgrade request after the drain begins, or over the HTTP server's `max_inflight`, gets the HTTP
  server's 503 before the hand-off sees it (`crates/ulo-http/src/service.rs:148-153`, before `route`
  hands upgrades over at `:214-218`). `WsModule`'s doc, the public face of the hand-off, carries
  the same sentence, and `GatewayTable::handshake`'s doc qualifies its 503.
- **The scenario:** the status-only assertion on `Port::Http` carries a comment pointing to that rule
  and F368 (`crates/ulo-ws-conformance/src/cases/handshake.rs`, `draining`).
- No code changed for F368.

### 7. S5: acceptance shown by a request on the same connection, and what both hosts do with it

- **The scenario as directed:** a connection carries `GET /nowhere`, answered 404 by either server
  (the standalone server's handshake finds no gateway; on the HTTP port the HTTP application has no
  route), which shows the server accepted it. An idle `/echo` WebSocket connection opens after that,
  with no order between the two assumed; the close starts; its 1001 on the idle connection shows the
  drain has begun; the upgrade request then goes on the first connection. The d3 sequence, the
  pending connection opened before the idle one and its head split around the 1001, is gone.
- **The probe against both hosts with nothing declared** (`probes/s5-keeps-idle-*.txt`): each read
  met the end of the stream after the upgrade request, "reading the response failed after "":
  unexpected end of file". A second ordinary request sent before the drain was answered 404 on both
  (`probes/s5-second-request-*.txt`), so neither closes after a refusal: both close a connection
  that is idle when the drain begins. hyper's graceful shutdown closes an idle connection
  (`conn.rs:886-894`), and a head arriving after a finished request leaves it idle until the head
  parses (`conn.rs:293`). `ulo-hyper-serve` starts that shutdown on every connection at the drain,
  for the standalone server and, through `ulo-http-hyper`'s `drive!`, for the HTTP server.
- **The declaration:** `Host::CLOSES_IDLE_AT_DRAIN`, `false` unless declared, states that property,
  and both hosts declare it with a comment naming the graceful shutdown. On a host keeping the
  connection the scenario asserts the 503, the handshake's plain-text refusal on `Port::Own` and the
  status alone on `Port::Http` (F368). On a host declaring it, the scenario reads the connection to
  its end, which must come with nothing written and within half the drain window (`DRAIN / 2`, two
  seconds): a server that keeps the connection until the window runs out and is then aborted has
  not closed it when its drain began. A first version required the app's close to be still running
  at the end of the stream instead; with the standalone server's graceful shutdown removed it
  passed, the abort ending both at once.
- **What no host now asserts on the wire:** the handshake's 503 for a request on an accepted
  connection. The table's decision is pinned at the hub by a new `tests/table.rs` test, a handshake
  once the drain has begun refused 503 "the server is shutting down" before any connection phase.
  F372 records the gap and the reason a hyper host reaches the 503 only through a connection's
  first request. See S5.
- **The client** reads a response that switches no protocol and keeps the stream
  (`read_response`), sharing the head and body readers `read_answer` uses; `within_for` is `within`
  with a bound of its own.

### 8. S7: the record corrected

The code, the suite and `ulo-ws`'s docs already say `Unimplemented` for an event nothing handles
(`connection.rs`, `message`; `NoHandler`'s doc; `cases/messages.rs`), as the transports DESIGN
does. The one text stating `NotFound` for it is the d3 log: its decision 5 bullet now states the
kind and records the brief's `NotFound` as ruled wrong, and its S7 carries the thirty-eighth
response's settlement. The break row "an unknown event as `NotFound`" names a deliberate break and
stays.

## The tests

- **`crates/ulo-ws/tests/table.rs`, five new,** over two new gateways that record each hook,
  `Paired` (`refuse = handshake`, `max_connections = 1`) and `Loose` (`refuse = close`), and a host
  whose write of the 101 errors, a `tokio::io::duplex` whose client end is dropped first:
  - `a_switch_dropped_when_its_101_fails_to_write_runs_on_disconnect_once_as_lost`: the write
    fails, the `Switch` is dropped, and after the app's close the record for that connection is
    `[Connected, Disconnected(Lost)]`.
  - `an_upgrade_that_fails_after_the_switch_is_served_runs_on_disconnect_once_as_lost`: hyper's
    shape, `serve` handed an upgrade future that fails after the failed write; the same record.
  - `a_failed_101_before_the_connection_phase_runs_no_hook`: on `Loose`, one `Switch` dropped and
    one served a failing upgrade; no hook ran.
  - `a_connection_closed_over_max_connections_after_its_on_connect_runs_on_disconnect`: a first
    connection holds the slot, shown by an answered `echo`; the second, admitted in the handshake,
    is closed 1013 and records `[Connected, Disconnected(ServerClose { code: 1013 })]`.
  - `a_handshake_once_the_drain_has_begun_is_refused_503_before_the_connection_phase` (S5): after
    the app's close, `/plain` and `/paired` are each refused 503 "the server is shutting down", and
    no hook ran.
  - Changed: `a_switch_dropped_unserved_takes_its_admitted_connection_out_of_the_rooms` reads the
    rooms after the app's close, the release now following `on_disconnect` on a tracked task.
- **`crates/ulo-ws-hyper/tests/abandoned_handshake.rs`,**
  `a_client_gone_while_on_connect_runs_gets_on_disconnect_once_as_lost` (F371): the client
  half-closes while `OnConnect` waits at a gate and reads the server's end of the connection, the
  positive signal that hyper ended the request, before the gate opens; `on_disconnect` runs once
  with `Lost`, nothing else runs by the app's close, and the room is empty.
- **`crates/ulo-ws-hyper/tests/prepare.rs`,**
  `the_server_s_timeouts_come_before_the_table_s_failures_and_the_endpoints_after` (S12): zero
  `header_timeout` and `handshake_timeout`, `max_inflight(Count::Max(0))`, an endpoint that does not
  parse and no gateway; the five failures appear in that order in the `Configure` report.
- **`crates/ulo-transport/src/watch.rs`,** `a_change_between_the_read_and_the_wait_wakes_the_waiter`
  (S4): a writer's change and notification modelled inside `ready`, after the read, where the lock
  is held; one poll of `wait_for` with a no-op waker must answer `Ready`. See S6.
- **`crates/ulo-rpc/tests/runtime.rs`** (S3): `a_zero_client_timeout_is_refused_where_it_is_written`
  is no longer `should_panic`: the `Err` names the link and carries the full sentence. New,
  `a_zero_module_timeout_fails_wiring_as_the_same_refusal`: `wire()` fails with a
  `WiringError::ValueFailed` whose error downcasts to `ZeroTimeout`, its sentence naming
  `RpcClientModule::timeout`. The nonzero bounds are `a_client_built_outside_an_app_times_out_at_its_own_timeout`'s,
  250 ms and `Unbounded`, now taken through `expect`.
- **`handshake_refuses_during_the_drain`** (S5), rewritten as decision 7 describes, on both hosts.

### Before and after

Each break went through `batch20/scripts/brk.py`, which replaces each span (asserting it occurs
once), runs the target, writes every file back byte for byte with `Cargo.lock` among them, and
compares a hash over `git diff HEAD` of `crates`, `Cargo.toml` and `Cargo.lock` and every untracked
file under `crates`; every restore reported `ok`, the hash unchanged. Specs are in `batch20/specs/`,
full output in `batch20/broken/` and the probes' in `batch20/probes/`.

| Break | Target | Result |
| --- | --- | --- |
| `Switch`'s `Drop` unregistering only, as at `430101b8` | `ulo-ws --test table` | the dropped-`Switch` test failed; six passed |
| a failed upgrade unregistering only | same | the failed-upgrade test failed; six passed |
| the slot refusal unregistering only | same | the `max_connections` test failed; six passed |
| the connection phase before the 101 for every gateway (`Refuse::Close` taking the handshake arm) | same | the `refuse = close` test failed, hooks recorded; six passed |
| the connection phase awaited inline, as at `430101b8` | `ulo-ws-hyper --test abandoned_handshake` | failed, "`on_disconnect` did not happen within 5s" |
| the handshake's drain check removed | `ulo-ws --test table` | the drain test failed, `/plain` accepted; seven passed |
| the table's refusals pushed before the server's timeouts | `ulo-ws-hyper --test prepare` | failed, "the report lists its failures out of order", at `[383, 541, 79, 243, 700]` |
| `wait_for` reading the value before it registers | `ulo-transport --lib watch` | failed, "a change made after the value was read and before the wait began was missed" |
| `RpcClient::timeout` taking a zero | `ulo-rpc --test runtime` | the client zero test failed, "was taken"; six passed |
| `RpcClient::timeout` refusing any bound but `Default` | same | the timeout test failed at 250 ms, "a nonzero timeout is taken: ZeroTimeout { .. }"; six passed |
| the module recording its refusal as text | same | the module test failed, "no wiring error carries a `ZeroTimeout`"; six passed |
| the module taking a zero | same | the module test failed, "with a zero timeout wired"; six passed |
| the standalone server keeping idle connections (no graceful shutdown at the drain) | `ulo-ws-hyper` conformance, the drain scenario | failed, "did not happen within 2s" |
| the HTTP server keeping idle connections (`drive!` without it) | `ulo-ws` conformance, the drain scenario | failed the same way |
| either host's `CLOSES_IDLE_AT_DRAIN` removed | each host's drain scenario | failed, the read meeting the end of the stream, on the final scenario |
| a broken link in `ulo-transport`'s crate doc | `cargo doc -p ulo-transport` under `-D warnings` | error, "unresolved link to `NoSuchItem`" |
| the restored tree | every target above | every test passed |

The first form of the closes-idle branch, which required the app's close to be still running, let
the standalone keep-idle break pass (`broken/s5-standalone-keeps-idle.txt`, 4.01 s) while catching
the HTTP one; the `DRAIN / 2` bound replaced it and both keep-idle breaks failed
(`broken/s5-*-keeps-idle-2.txt`).

## Left for the transports DESIGN fold

Line numbers are those read on `430101b8`.

- §4.1, X32 (line 755): "A `Switch` dropped unserved ..." reads: a `Switch` dropped unserved, or
  served an upgrade future that fails, runs `on_disconnect` with `Lost` for the connection its
  handshake admitted, then releases it, on a task the drain counts; a connection over
  `max_connections` admitted in the handshake gets `ServerClose { code: 1013 }`. The connection
  phase under `refuse = handshake` runs as a task on the table's tracker, so a server that drops the
  handshake's future leaves it to finish (F371). The rule to state once: every connection whose
  `OnConnect` ran gets `on_disconnect`.
- §4.1, line 813: the tracker's `Watch` is `fw-transport`'s, shared with `fw-rpc` through
  `__private`, not a copy; §5.3's server paragraph likewise.
- §4.1, the conformance paragraph (line 757): `Host` gains `const CLOSES_IDLE_AT_DRAIN: bool`,
  `false` by default, which both shipped hosts declare.
- §4.1, line 761: the drain scenario's description reads as decision 7: acceptance shown by a 404
  on the same connection, then on a host keeping it the 503, on one closing it the connection's end
  within half the drain window; no arrival order assumed. Line 763's last sentence, on a server
  accepting out of arrival order declaring the scenario not applicable, no longer applies; its
  F368 sentence gains that both current hosts take the closing branch (F372).
- §3.5 (line 486): the hand-off's rule, "on the HTTP port, the HTTP server's drain and load
  shedding answer first" (F368, option (a)).
- §5.4 (line 1024): `RpcClient::timeout(Bound) -> Result<Self, ZeroTimeout>`; a zero is refused
  where it is written as `ZeroTimeout`, the error `RpcClientModule` records at `wire()`.
- §11, X29 (line 1319): "a zero panicking where it is written" reads "a zero refused as
  `ZeroTimeout`"; `ZeroTimeout` joins the row.
- §12's refusal table (line 1383): the row for a client's zero timeout is a `ZeroTimeout` from
  `timeout`, not a panic.
- Decision 61 (line 1482) is superseded on its last clause: an error answered from the builder,
  as the thirty-seventh response's S3 directs.

## Needs sign-off

Numbered from S1 for this batch; the items it builds are named by their logs.

### S1. A connection closed over `max_connections` after its `OnConnect` gets `on_disconnect`

Decision 1. Under `refuse = handshake` the slot is taken after the upgrade, so a connection
`OnConnect` admitted can still be closed 1013; it now gets `on_disconnect` with
`ServerClose { code: 1013 }` after the close frame. The alternatives are taking the slot before the
connection phase, refusing the handshake before `OnConnect` runs, or leaving that path without
`on_disconnect`.

### S2. The connection phase is a task, one spawn per `refuse = handshake` upgrade

Decision 2. The phase finishes whatever the server does with the handshake's future, and the drain
waits for it; a phase aborted by the table's close answers the handshake 503. The alternatives are a
guard in `connect` that unregisters a dropped phase, which leaves `OnConnect` cut off mid-await and
its side effects unpaired, or documenting that a server dropping the decision cuts the phase off.

### S3. An unserved connection's rooms are released after its `on_disconnect`, not at the drop

Decision 1. `on_disconnect` sees the connection's rooms on every path, as on the served one, and
the release becomes asynchronous on the drop path: a room broadcast between the drop and the
release still reaches the connection's queue, which nothing writes. The alternative is releasing
at the drop and running `on_disconnect` after it, with the rooms gone.

### S4. `timeout` drops the handle with its `ZeroTimeout`

Decision 3. `RpcClient::new(link, rt).timeout(b)?` is the shape the response gives, and a builder
consumed by value cannot hand the client back in the `Ok` and the `Err` both without a wider error
type. The alternative is an error that carries the client back.

### S5. The drain scenario declares closing, and its 503 branch runs on no current host

Decision 7. Both hyper hosts close a connection idle when the drain begins, which the response's
sequence assumed they would keep; the declaration states what they do, the hub-level test pins the
table's 503, and F372 records that no host reaches it on the wire. The `DRAIN / 2` bound is a
failure limit, as `PATIENCE` is, not the acceptance proof. The alternatives are a host hook
reporting a connection accepted, under which the first request could cross the drain on hyper, or
d3's first-request path kept for hosts declaring they accept in arrival order, which the response's
"remove the workaround" rules out as built.

### S6. The `Watch` test reaches the private event

Decision 4. `wait_for` calls `ready` with the value's lock held, so the public API has no seam
between the read and the wait; the test models the writer inside `ready` by changing an atomic
value and notifying the event directly. A behaviour-preserving change to the wake-up mechanism
would need the test rewritten. The alternative is a statistical two-thread test, which the broken
order passes most of the time.

## Verification

Full, unfiltered output of every run is in the session scratchpad, `batch20/`: `verify/` for the
pass below, `runs/` for the runs during the build, `broken/` and `specs/` for the breaks, `probes/`
for the probes, `drain-loop/` for the hundred runs, and `scripts/`.

- **The WebSocket crates,** `cargo test -p <crate> --locked`, three runs each and a fourth on the
  final tree: `ulo-ws` 49 passed and 4 ignored (44 before, the five `table.rs` tests added),
  `ulo-ws-hyper` 70 and 1 (68 before, `abandoned_handshake` and `prepare` added),
  `ulo-ws-conformance` 1 and 2. Each host's suite is 29 scenarios.
- **The drain scenario alone,** 100 runs per host from copies of the built binaries
  (`drain-loop/`): standalone 100 of 100, HTTP port 100 of 100. The loop counts a run passed only on
  "1 passed; 0 failed"; run first with a name matching no scenario, it counted the run failed.
- **Other crates,** once each: `ulo-rpc` 17 passed and 5 ignored (16 before, the module test added);
  `ulo-transport` 1 and 1; `ulo-graphql-ws` 2; `ulo-rpc-tcp` 89 and 1 over 10 binaries, its three
  conformance suites among them; `ulo-rpc-udp` 30 and 2.
- **Broker suites,** once each, one at a time, `--features integration --test conformance --locked`:
  NATS 27 passed (5.57 s); Redis 28 (5.50 s); MQTT 32 (5.64 s); Kafka 27 (26.86 s); RabbitMQ with
  `ULO_CONFORMANCE_PARALLEL=6` 27 (19.99 s). `ulo-ws-redis`'s two-process test, whose tracker uses
  the shared `Watch`, 1 passed and `peer_instance` ignored (0.44 s), no peer process left.
- `cargo check --workspace --all-targets --all-features` with
  `CFLAGS=-I/opt/homebrew/opt/openssl/include LDFLAGS=-L/opt/homebrew/opt/openssl/lib`, and with
  defaults: exit 0, the 17 known warnings in `crates/ulo/src` and no other location
  (`scripts/diag.py` over the JSON messages; it reported an unused function in `client.rs` while the
  scenario was being written, before the call existed).
- `cargo +1.88 check --workspace --all-targets --exclude ulo-http-salvo --exclude
  ulo-graphql-async-graphql`: exit 0, the same 17 and no other.
- `cargo test --workspace --no-fail-fast` with the OpenSSL flags: 621 passed, 0 failed, 71 ignored
  across 160 test binaries: e1's 612, the five `table.rs` tests, `abandoned_handshake`, `prepare`,
  the module zero test and the `Watch` unit test.
- **The tree checks,** `cargo tree -p <crate> -e normal -i tokio --locked`: stdout is empty for
  `ulo`, `ulo-transport`, `ulo-net`, `ulo-http` (defaults), `ulo-rpc`, `ulo-ws`, `ulo-graphql-ws`,
  `ulo-graphql-http`, `ulo-ws-conformance` and `ulo-smol`, each exiting 101 with "did not match any
  packages" or 0 with "nothing to print"; with `--all-features --target all` the same for
  `ulo-transport`, `ulo-rpc`, `ulo-ws` and `ulo-ws-conformance`. `ulo-ws-hyper` prints tokio through
  hyper, the positive control.
- `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --lib -p <crate>` for `ulo-transport`, `ulo-rpc`,
  `ulo-ws`, `ulo-ws-hyper` and `ulo-ws-conformance`: each exits 0; decision 4 records what the
  check covers.
- `cargo +1.98.1 clippy -p ulo-transport -p ulo-rpc -p ulo-ws -p ulo-ws-hyper -p ulo-ws-conformance
  -p ulo-rpc-tcp --all-targets --no-deps --locked`: exit 0, 34 warning locations, 16 outside
  `crates/ulo/src`, none on a line this batch changed or in a new file, by
  `scripts/changed_lines.py` over `git diff -U0 HEAD` and the untracked files; given two existing
  locations as planted changed lines, it reported both.
- **After the pass,** the suite's host example in `crates/ulo-ws-conformance/src/lib.rs`, an
  `ignore` doc example, gained the `CLOSES_IDLE_AT_DRAIN` line; the three WebSocket crates' tests
  and the workspace check ran again on that tree with the counts above.
- **Containers:** the user's four, seaweedfs, mailpit, postgres:18 and redis:7, before and after
  every run and untouched. Every container a broker suite or the Redis test started was removed by
  the test that started it. No permission check refused an action.
