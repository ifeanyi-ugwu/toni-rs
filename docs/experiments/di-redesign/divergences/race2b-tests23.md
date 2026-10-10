# Divergences: race 2b, tests batch 23, a read-connections hook for the WebSocket and HTTP suites closing F372 and F373 (S5), the slot taken between the guards and `OnConnect` (S1), WebSocket's two in-flight bounds (S11), the permit-before-`Ack` order probed (S15), one close reason (S13), F376 fixed, `ordered_control` (S4), and `WsModule`'s init hook (S4b)

The forty-second, forty-third and forty-fifth responses, signed off 2026-10-10, answer
`race2b-tests20.md`, `race2b-tests19.md` and `race2b-tests22.md`. Both suites' `Host` gains
`connections_read`, which `ulo-hyper-serve` answers from a `ReadCount` it keeps; the count is of
connections read from rather than accepted, since hyper's graceful shutdown closes a connection it
has read nothing from (F377). The WebSocket drain scenario sends half an upgrade head on a fresh
connection, waits for the count, begins the drain and expects the 503 on both hosts (F372 closed);
the HTTP drain scenario drops its arrival-order proof and `STOP_SETTLES` for the same count (F373
closed), rocket declaring it not applicable (F378). Each ran 100 times per host. A WebSocket
connection's slot under `max_connections` is taken where its guards have admitted and before
`OnConnect`, so a connection refused for capacity runs no hook. A connection's own `max_inflight`
is 64 again and a server-wide `server_max_inflight` of 1,024 joins it. The permit-before-`Ack`
order fails `over_the_bound` on RabbitMQ and Kafka once the window is widened. WebSocket's 1001
reason is "the server is shutting down". A streamed request cancelled before its `opened` sends
its `cancel` once the `opened` arrives, on all four broker links (F376 closed, F379 filed).
`Capabilities::ordered_control` gates the stamped `cancel_follows_its_request`, and
`WsModule`'s init hook calls the broadcast adapter's `prepare`. Running the stamped scenario found
F380: batch 22's writers let a `cancel` overtake its request, on TCP 16 runs in 100. It is fixed
here: TCP, UDP, Redis, RabbitMQ and Kafka feed their writers through one queue in
`ulo-transport`'s `__private` that fixes a frame's place at the send's call.

Files changed: `crates/ulo-hyper-serve/src/{lib.rs, serve.rs, listener.rs, handshake.rs}`;
`crates/ulo-http-hyper/src/{lib.rs, backend.rs}`; `crates/ulo-grpc/src/server.rs`;
`crates/ulo-ws-hyper/{src/lib.rs, tests/conformance.rs, tests/handshake.rs, tests/hooks.rs,
tests/limits.rs, tests/prepare.rs}`; `crates/ulo-ws/{src/broadcast.rs, src/connection.rs,
src/gateway.rs, src/handoff.rs, src/module.rs, src/table.rs, tests/conformance.rs,
tests/handoff.rs, tests/table.rs}`; `crates/ulo-ws-conformance/src/{lib.rs, cases/mod.rs,
cases/handshake.rs, cases/close_codes.rs}`; `crates/ulo-ws-redis/{src/lib.rs, tests/handle.rs}`;
`crates/ulo-transport/src/count.rs`; `crates/ulo-http-conformance/src/{lib.rs, wire.rs,
reference.rs, cases/drain.rs, count.rs (new)}`; the conformance hosts of axum, actix
(`tests/common/mod.rs`), salvo, poem and rocket; `crates/ulo-rpc/src/link.rs`;
`crates/ulo-rpc-conformance/src/{lib.rs, cases/order.rs}`; `src/link.rs` of the TCP, Redis,
NATS, RabbitMQ, MQTT and Kafka links and `tests/conformance.rs` of all seven; in the workspace's
`crates/ulo-transport/src/{lib.rs, __private.rs, ordered.rs (new)}` and the UDP link's
`src/link.rs`, for F380; in the workspace's `FRAMEWORK_GAPS.md`, notes under F372, F373, F376 and
F380, and F377 to F380 filed. `Cargo.lock`
does not change. The tree is `7cfd944b` plus this batch.

## The signatures

```rust
// ulo_hyper_serve, new
#[derive(Clone, Debug, Default)]
pub struct ReadCount(/* private */);
impl ReadCount { pub fn get(&self) -> usize; }
pub struct ServeConfig {
    pub handshake_timeout: Option<Duration>,
    pub read_count: ReadCount,                 // added
}

// ulo_http_hyper and ulo_ws_hyper, new; both re-export `ReadCount`
impl Hyper { pub fn read_count(&self) -> ReadCount; }
impl ulo_ws_hyper::Server {
    pub fn read_count(&self) -> ReadCount;
    pub fn server_max_inflight(self, messages: Count) -> Self;
}

// ulo_ws, new
impl GatewayDefaults { pub fn server_max_inflight(self, messages: Count) -> Self; }
impl WsModule { pub fn server_max_inflight(self, messages: Count) -> Self; }

// ulo_ws_conformance, changed
pub trait Host: Sized + 'static {
    fn bind(app: App<Connected>) -> (App<Connected>, Self);   // was `-> App<Connected>`
    fn connections_read(&self) -> Option<usize>;              // new, required
    /* .. */
}

// ulo_http_conformance, changed and new
pub trait Host { fn connections_read(&self) -> Option<usize>; /* new, required */ }
#[derive(Clone, Debug, Default)]
pub struct ReadCount(/* private */);
impl ReadCount { pub fn get(&self) -> usize; pub fn wrap<S>(&self, stream: S) -> Counted<S>; }
pub struct Counted<S> { /* private */ }     // tokio's `AsyncRead` and `AsyncWrite` where `S` has them

// ulo_rpc::Capabilities, added
pub ordered_control: bool;                      // `false` from `new`
pub const fn ordered_control(self, ordered_control: bool) -> Self;

// ulo_transport::__private::ordered, new, doc-hidden
pub fn channel<T>(bound: usize) -> (Sender<T>, Receiver<T>);
impl<T> Sender<T> {                                  // Clone
    pub fn send(&self, item: T) -> Room<T>;          // queues now; `Room` resolves within the bound
    pub fn try_send(&self, item: T) -> Result<(), TrySendError<T>>;
}
impl<T> Future for Room<T> { type Output = Result<(), Closed>; }
impl<T> Receiver<T> {
    pub fn recv(&mut self) -> impl Future<Output = Option<T>> + '_;
    pub fn poll_recv(&mut self, cx: &mut Context<'_>) -> Poll<Option<T>>;
}
pub struct Closed;
pub enum TrySendError<T> { Full(T), Closed(T) }

// ulo_rpc_conformance, stamped scenarios
pub mod cases { pub mod order {
    pub async fn cancel_follows_its_request<B: Broker>();     // was called by hand from two crates
    pub async fn cancel_before_opened<B: Broker>();           // new
} }
```

What changes in meaning without a signature: a WebSocket gateway's `max_inflight` at
`Count::Default` is 64 again; `WsModule` runs an init hook calling the adapter's `prepare`; the
1001 close reason at the drain and for a connect refused once the app is closing reads "the server
is shutting down"; a connection over `max_connections` runs no hook in either refusal mode and no
longer gets `on_disconnect` with `ServerClose { code: 1013 }`; under `refuse = close` a connect
guard decides before capacity, so a refused connection on a full gateway closes with the guard's
1008 rather than 1013.

Private: in `ulo-ws`, `Places` (the server's in-flight places, on `Tracker`), `Decided` (on
`Switch`), `Connected::Full`, `Admitted::slot`, `Flight::new` and `Flight::begin -> bool`,
`received -> Option<Frame>`, `Tracker::new(limit)`, `GatewayTable::new(.., defaults)`,
`DEFAULT_CONNECTION_INFLIGHT`, `TOO_MANY`; in `ulo-hyper-serve`, `Io` gains a field and `new` and
`counting`; on NATS, RabbitMQ, MQTT and Kafka, `ClientCall::Cancelled`; the NATS and MQTT pumps.
`WsModule`'s `SharedAdapter` forwards `prepare`.

## Decisions

### 1. S5: the hook counts reads, not accepts

- **The first form counted accepts and failed on hyper.** `ulo-hyper-serve` counted each
  connection as it handed it to hyper. `drain_http1` on the hyper reference then failed both
  modes at once, "the request is written: Broken pipe": the count included the connection before
  hyper had read its half head, and hyper's graceful shutdown closes a connection still in its
  initial state, nothing read and nothing written (`hyper-1.11.1/src/proto/h1/dispatch.rs:91-100`,
  `conn.rs:195-199`). Poem failed one mode the same way. The response's premise, that a connection
  whose handshake has started is busy, holds once hyper has read from it, not once it is accepted.
  See S1 and F377.
- **The count is taken at the first read that yields bytes:** `Io::poll_read` counts once,
  after the TLS handshake where there is one, in the task that polls hyper's connection, so the
  bytes are in hyper's read buffer before the drain signal can reach the same `select!`.
- **One accessor, one type.** `ServeConfig::read_count` carries a `ReadCount` in; each server
  keeps a clone from its construction and hands it out through `read_count()`, since the server
  has moved into the app by the time a scenario reads it. gRPC's server passes the default and
  exposes nothing.
- **The embed hosts.** axum, salvo and poem wrap each accepted stream in the HTTP suite's
  `ReadCount::wrap` through their own acceptor or listener; axum's goes under a no-op `tap_io`,
  the one wrapper axum gives `ConnectInfo<SocketAddr>` for. actix takes a listener and gives no
  hold on a stream, so its host counts in `on_connect`, as its worker starts each connection; its
  `DrainPending::Closed` branch never finishes the head and asks only that the connection was
  accepted. rocket 0.5 binds and accepts inside `launch` and answers `None` (F378).
- **`None` fails the scenario** with a message naming the declaration, and a host that cannot
  count declares the scenario not applicable, the suites' rule for a scenario that cannot apply.
- **The WebSocket `Host` hands back a value.** Its methods were all associated functions on a
  stateless type; `bind` now returns `(App<Connected>, Self)`, the host value carrying the count,
  so `connections_read` is a method as on the HTTP suite's `Host`.

### 2. S5: the two scenarios

- **WebSocket drain:** the connection idle between requests (a 404 answered on it) and an idle
  WebSocket connection open first; then a fresh connection carries the upgrade head up to its
  `Sec-WebSocket-Key` line, the scenario waits for `connections_read` to pass its earlier value,
  starts the close, waits for the idle WebSocket's 1001, sends the rest and expects 503: the
  handshake's plain-text refusal on `Port::Own`, the status alone on `Port::Http` (F368). The idle
  connection then takes the `CLOSES_IDLE_AT_DRAIN` branch as before. F372 is closed: the 503
  branch runs on both hosts.
- **HTTP drain:** `drain_http1` writes half a head, waits for the count, begins the drain and, on
  a `Served` host, finishes the head at once. The app's 503 carries `Connection: close` itself
  (`crates/ulo-http/src/render.rs:87-92`), so no wait for the host's stop is needed. F373 is
  closed.
- **The RPC suite takes no hook.** Its drain scenarios open their client connection before the
  drain and wait `settle` for a held call to reach its handler; neither infers acceptance from
  arrival order (`crates/ulo-rpc-conformance/src/cases/drain.rs`).

### 3. S1: guards, then the slot, then `OnConnect`

- **Where the slot is taken:** inside the call `connect` hands `ulo::dispatch`, which runs once
  every connect guard has admitted and before the handler's `OnConnect`. A free slot is kept for
  the connection; none sets a flag, the call answers a refusal of kind `Unavailable`, and
  `connect` returns `Connected::Full` whatever an interceptor made of that answer. The session
  and the hub registration still come first, as before; neither is a hook. See S2.
- **An interceptor that answers `Admitted` without calling the handler** takes no slot in the
  call; `connect` takes it after `dispatch`, and `Full` if none is free.
- **Both refusal modes share it.** Under `refuse = close` the phase runs after the 101 and a full
  gateway closes 1013. Under `refuse = handshake` the phase runs before the 101 and a full gateway
  answers a `Switch` decided `Full`, which upgrades and closes 1013, as the capacity refusal was
  written before; the conformance scenario `max_connections_closes_the_next_with_1013` reads the
  same close.
- **The slot lives in `Admitted`** and is freed after `on_disconnect` and the unregistering, the
  order the served path had.
- **What changed for `refuse = close`:** the slot used to be taken right after the 101, before
  the guards, so a guard-refused connection on a full gateway closed 1013. It now closes with its
  guard's 1008, as W 28 settled for the handshake mode.

### 4. S11: two bounds, one mechanism

- **Per connection:** `DEFAULT_CONNECTION_INFLIGHT`, 64, in `Limits::resolve`.
- **Server-wide:** `server_max_inflight`, `Count::Default.max_inflight()` (1,024), resolved once
  per table into `Places` on the table's `Tracker`. A message takes a place where it takes its
  count under the connection's `max_inflight` (`Flight::begin`) and gives it back where it leaves
  that count (`Flight::handled`); a hand-written gateway's frames still queued when the connection
  ends give theirs back as the `Flight` drops.
- **Stopping reading:** each connection's read loop reads only while the server has a free place,
  besides its own bound. A message read as the last place went, by another connection between this
  one's check and its read, waits on the connection unhandled, the loop reading nothing more until
  a place frees; a `cancel` and a frame refused before dispatch take no place. A freed place with
  none left before wakes every connection waiting. The handlers in flight never exceed the bound,
  and each connection holds at most one message read beyond it. See S3.
- **Exposed:** `GatewayDefaults::server_max_inflight`, `WsModule::server_max_inflight` and
  `ulo_ws_hyper::Server::server_max_inflight`; `Count::Max(0)` is refused in `prepare`. It is a
  server's setting, so the gateway attribute has none.
- **The name:** the per-connection setting keeps `max_inflight`, which the attribute and both
  builders already carry. See S4.

### 5. S15: probed, and the comment kept

`Call::run` already carries a comment at the spot naming why the permit is freed first
(`crates/ulo-rpc/src/dispatch.rs`, the `answered` arm). The widened-window probe was cheap and is
recorded in the tests below: with the order reversed and 300 ms between the settlement and the
permit's release, `over_the_bound` refused one call on RabbitMQ and two on Kafka; with the order
reversed alone it passed on RabbitMQ, as batch 19 found. The comment stays as written. A probe
cannot stay in the tree without a hook in the dispatcher, so nothing pins the order permanently.
See S5.

### 6. S13: one phrase

The 1001 reason the drain writes to an idle connection, and the one a connect refused once the
app is closing carries, read "the server is shutting down". Four tests pinning the old text
changed: the suite's `drain_closes_with_1001` and the drain scenario's wait for the idle 1001,
`ulo-ws`'s `the_drain_closes_a_same_port_connection_with_going_away` and `ulo-ws-hyper`'s hooks
test.

### 7. F376: the `cancel` held for the `opened`

- **RabbitMQ and Kafka:** a `cancel` for a streamed request still waiting for `opened` replaces
  its entry with `ClientCall::Cancelled`; the writer's `Job::Opened` answers that entry with the
  `cancel` alone and removes it. The `in` and `in_end` held before are dropped, the call being
  over; frames sent after the `cancel` find `Cancelled` and are dropped.
- **NATS and MQTT:** the `cancel` goes onto the call's pump queue and the entry becomes
  `Cancelled { gate }`; the `opened` opens the gate and removes the entry, and the pump, finding a
  `cancel` among what was queued before the gate opened, publishes it alone.
- **"Goes out first":** the response asks for the held `cancel` first and in order. It goes out
  alone; nothing held before it is sent after it.
- **An `opened` that never comes** leaves the entry until the link closes (F379). An `open` the
  broker refuses outright removes it, as before.

### 8. S4: `ordered_control`

`Capabilities::ordered_control`, `false` from `new`, is declared by TCP and Redis.
`cancel_follows_its_request` is stamped into every suite and fails, naming the declaration, on a
link that does not declare the capability; NATS, RabbitMQ, MQTT and Kafka declare it not
applicable citing U17, and UDP because datagrams arrive in any order. The payload is encoded in
the link's codec, since the TCP CBOR suite now runs the scenario. `cancel_before_opened`, F376's
scenario, is stamped too: it runs on every link carrying a streamed request and passes trivially
where a request and its control frames share one lane; UDP declares it not applicable.

### 9. F380: found by the stamped scenario, and fixed

- **The finding.** Run 100 times alone, `cancel_follows_its_request` on TCP failed 16 runs on
  this tree and 17 on `7cfd944b`, built in a scratch worktree since removed; each failure held one
  or two requests with no `cancel` after them. A probe in that worktree logged the server dropping
  a `cancel` that named no call while the client had pushed it one place before its request
  ("client pushed request at 103, cancel at 102"). Each send waited on tokio's bounded `mpsc` for a
  permit and pushed when it got one; by reading, a request waiting for room has its permit
  assigned while its task is not running, and a `cancel` polled next takes a place the writer
  freed and pushes first.
- **The fix: one queue, `ulo_transport::__private::ordered`.** `Sender::send` puts the item in the
  queue under the lock at the call and answers a `Room` future that resolves once the item is
  among the first `bound` the receiver has not taken; the bound is applied after the place is
  fixed, so waiting for room cannot reorder. Runtime-free (a `std` mutex and `event-listener`, as
  `Watch`), cancel-safe on the receiving side, and an item stays queued when its `Room` is dropped.
  A dropped receiver fails the waiting sends `Closed` and refuses new ones; `try_send` refuses past
  the bound, for TCP's `goaway`.
- **Why one shared queue rather than an ordered channel with a semaphore beside it.** A semaphore
  acquired in order fixes the place at the acquire, which is the poll, and a credit the writer
  returns needs the same ticket at the call; either still needs the queue to take the item at the
  call. The queue does both in one place, and the five links had five copies of the mpsc pattern.
- **At the call, not the poll.** Each link's client `send` encodes the frame and queues it in the
  closure's body, before the `BoxFuture` it returns is polled; each reply path does the same,
  forgetting a finished call's id at the queueing; the server writers' `write` helpers are plain
  functions that queue, then return the wait. The instruction's invariant, the place fixed at the
  call, is therefore what the five links keep; `Outbound`'s doc says so and scopes it to them.
- **Kept from batch 22:** confirmations and delivery reports waited on beside later publishes,
  Redis awaiting each `PUBLISH`, held frames released on `Opened` (the router's `Job::Opened` goes
  through the same queue), a queued frame written after its send is dropped, the 64-frame reply
  read-ahead (`REPLY_QUEUE`, untouched) and the bound of 64.
- **NATS and MQTT** hand frames to their libraries' command channels and are not changed; their
  `cancel` travels a control lane of its own anyway (U17).

### 10. S4b: `WsModule`'s init hook

`WsModule::register` adds `m.on_init` calling the adapter's `prepare`, so `connect` fails with
`ConnectError::Hook` carrying the adapter's refusal, a gateway served or not. The hubs' `prepare`
still calls it, and `publish` keeps its refusal. With `WsModule` the only way to give an app an
adapter, the hubs' call can no longer refuse what `connect` did not; the two tests that pinned the
`listen()` refusal now pin the `connect` failure. See S6.

## The tests

- **S5:** `handshake_refuses_during_the_drain` on both WebSocket hosts and `drain_http1` on every
  HTTP host, as decisions 1 and 2 describe; rocket's is declared not applicable.
- **S1,** `crates/ulo-ws/tests/table.rs`:
  `a_connection_over_max_connections_admitted_in_the_handshake_is_closed_1013_with_no_hook`
  replaces batch 20's `ServerClose { code: 1013 }` test and asserts the record holds the holder's
  two hooks alone; new, `a_full_gateway_s_connect_guard_refuses_first_and_capacity_runs_no_hook`
  on `Capped` (`refuse = close`, `max_connections = 1`, a header guard): a guard-refused
  connection closes 1008, an admitted one 1013, and neither runs a hook.
  `crates/ulo-ws-hyper/tests/handshake.rs`'s capacity test changed only its comment.
- **S11:** `a_connection_holds_64_messages_in_flight_by_default` (66 messages on one connection:
  64 handlers start, the 65th after one answers, the most at once 64) and
  `the_server_s_connections_together_stop_at_its_bound_and_resume_as_places_free` (bound 3, one
  message on each of five connections, each answer letting one more start, the most at once 3) in
  `crates/ulo-ws-hyper/tests/limits.rs`; `a_zero_server_wide_in_flight_bound_is_refused` in
  `tests/prepare.rs`; `ws_module_s_server_bound_holds_the_http_port_s_connections_together`
  (bound 1, two connections) in `crates/ulo-ws/tests/handoff.rs`.
- **S13:** the four tests of decision 6.
- **F376:** `cancel_before_opened`, 20 streamed requests each cancelled in the poll of its `open`;
  the server's link must deliver every `open`, then its `cancel`, and hold no call.
- **S4:** `cancel_follows_its_request`, stamped, on TCP's three suites and Redis's.
- **F380,** `crates/ulo-transport/src/ordered.rs`: `a_send_waiting_for_room_keeps_its_place_ahead_of_a_later_one`
  (bound 1: a send waiting for room, the receiver freeing room, a later send polled before the
  waiting one runs again; the waiting send's item comes out first),
  `an_item_whose_wait_is_dropped_is_still_received`,
  `a_receiver_gone_fails_the_waiting_sends_and_refuses_new_ones` and
  `try_send_refuses_past_the_bound`; `cancel_follows_its_request` on TCP and Redis.
- **S4b:** `a_broadcast_adapter_refusing_in_prepare_fails_connect` (`crates/ulo-ws/tests/handoff.rs`)
  and `a_process_broadcasting_over_an_adapter_with_no_runtime_fails_connect`
  (`crates/ulo-ws-redis/tests/handle.rs`), each over a root importing `WsModule` and serving no
  gateway.

### Before and after

Each break went through `batch23/scripts/brk.py`, as in batch 22: it replaces each span
(asserting it occurs once) or restores a file to `HEAD`, runs the target, writes every touched
file and `Cargo.lock` back byte for byte, and compares a hash over `git diff HEAD` of `crates`,
`Cargo.toml`, `Cargo.lock` and `.github` and every untracked file under `crates` and `.github`.
Every restore reported `ok`, the hash unchanged: `a6b35d67213a` for the breaks before F380's fix,
`da3125b7fd8a` for those after it. Specs are in `batch23/specs/`, `specs2/` and `specs3/`, full
output in `batch23/broken/`.

| Break | Target | Result |
| --- | --- | --- |
| `ulo-hyper-serve` counting at the hand-off to hyper, the hook's first form | 20 runs each of `drain_http1` on the hyper reference and of the drain scenario on each WebSocket host | the reference failed 3 runs of 20, 4 tests of 40, "Broken pipe" or an empty answer where 503 was due; the WebSocket hosts passed 40 of 40 (F377) |
| the table's drain check removed | the standalone host's drain scenario | failed, the busy connection answered 101 |
| the HTTP server dropping its connections at the drain instead of shutting them down gracefully | the HTTP port's drain scenario | failed, "reading the response failed after \"\": unexpected end of file" |
| the standalone host answering `connections_read` `None` | its drain scenario | failed, the message naming the declaration |
| the hyper reference answering `None` | its `drain_http1` | both modes failed, the message naming the declaration |
| rocket's `drain_http1` declaration removed | rocket's `drain_http1` | both modes failed the same way |
| the slot taken after `OnConnect` | `ulo-ws --test table` | both capacity tests failed, the refused connection's `Connected` in the record |
| capacity checked before the guards under `refuse = close` | same | the guard test failed, 1013 where 1008 was due |
| the connection default at 1,024 | `ulo-ws-hyper --test limits` | the 64 test failed, 66 at once |
| `try_take` always taking | the limits and hand-off tests | the hand-off test failed, 2 at once against 1; the limits tests passed, the connections still stopping reading at the bound |
| `is_full` always `false` | same | both passed: `try_take` alone holds the handlers to the bound |
| both removed | same | both failed, 4 at once against 3 and 2 against 1 |
| a freed place waking nobody | `ulo-ws-hyper --test limits` | the server test failed, "4 handlers starting did not happen within 5s" |
| `WsModule::server_max_inflight` dropping its value | `ulo-ws --test handoff` | the hand-off test failed, 2 at once |
| the zero `server_max_inflight` refusal removed | `ulo-ws-hyper --test prepare` | the zero test failed, the server listening |
| the old 1001 reason | `drain_closes_with_1001` on both hosts | both failed, `"server shutting down"` |
| `WsModule`'s init hook removed | the two S4b tests | both failed, the app connecting |
| TCP not declaring `ordered_control` | TCP's `cancel_follows_its_request` | failed, the message naming the declaration |
| NATS's declared scenario run with `--ignored` | NATS's `cancel_follows_its_request` | failed the same way |
| each broker link's `link.rs` as at `7cfd944b` | its `cancel_before_opened` | failed on NATS, RabbitMQ, MQTT and Kafka, "20 opens and 0 cancels of 20 each reached the server within 5s of the last" |
| the permit freed after the `Ack` | RabbitMQ's `over_the_bound` | passed |
| the same with 300 ms between them | RabbitMQ's and Kafka's `over_the_bound` | both failed: RabbitMQ served 5 of 6, Kafka 4 of 6, the rest refused "the server is over its in-flight limit" |
| the ordered queue reshaped to push once room is free, a bounded channel's shape | `ulo-transport --lib ordered` | the race test failed, "the late item waits behind the early one", and two others with it |
| TCP's `link.rs` as at `HEAD`, `ordered_control` declared, built once and run from a copy | `cancel_follows_its_request` | 2 of 100 failed unloaded, 17 of 200 under ten CPU-bound `yes` processes; the fixed link passed 200 of 200 under the same load |
| TCP's `link.rs` as at `HEAD` without the declaration, the break's first form | same, 30 runs | all failed on the capability assertion; kept as `broken/f380-tcp-at-head-invalid-no-capability.txt` and not counted |

## Left for the transports DESIGN fold

Line numbers are those read on `7cfd944b`.

- §1's `fw-hyper-serve` row (line 27) and §3.7 (line 558): the loop counts the connections it
  hands to the server at their first read into a `ReadCount` from `ServeConfig`; the HTTP backend
  and the standalone server expose it as `read_count()`.
- §3.8's conformance paragraph (line 696): `Host::connections_read`; `drain_http1` waits for it
  and has no fixed wait; rocket declares `drain_http1` not applicable (F378); the suite offers
  `ReadCount::wrap` for an embed host's acceptor.
- §4.1's conformance paragraphs (lines 758-766): `Host::bind` returns the host value and
  `Host::connections_read` is required; line 762's drain description reads as decision 2, the 503
  asserted on both hosts, and its arrival-order sentence is gone; the 1001 reason is "the server is
  shutting down".
- §4.1, X32 (line 756) and the hooks list (line 775): a connection over `max_connections` runs no
  hook, the slot taken between the guards and `OnConnect` in both refusal modes; batch 20's
  `ServerClose { code: 1013 }` path is gone.
- §4.1's load shedding (line 812): `max_inflight` is per connection, 64 at `Default`;
  `server_max_inflight`, 1,024, bounds the server's connections together by the same mechanism,
  a message read as the last place went waiting for one; settings vocabulary (line 810).
- §4.1's `WsModule` paragraph (line 768) and §4.3 (line 840): the module's init hook calls the
  adapter's `prepare`, failing `connect`.
- §10 (line 1284) and decision 56 (line 1477): the 1001 close reason is the one phrase too; the
  exception sentence goes.
- §5.2's broker paragraph (line 896): a `cancel` before `opened` is held and sent once the
  `opened` arrives, the frames held before it dropped (F376).
- §5.2's conformance paragraph and scenario list (lines 908-912): `cancel_follows_its_request`
  stamped and gated on `ordered_control`; `cancel_before_opened`.
- §5.3's `Capabilities` block (line 978): `ordered_control: bool`.
- §5.3's writer rule, and the `Outbound` doc it quotes: TCP, UDP, Redis, RabbitMQ and Kafka queue
  each frame at the send's call through `ulo-transport`'s ordered queue, the bound of 64 applied
  after the place is fixed; the order is the order of calls, not of first polls (F380).

## Needs sign-off

Numbered from S1 for this batch; the items it builds are named by their logs.

### S1. The hook counts connections read from, not accepted

Decision 1. A count of accepts let `drain_http1` race hyper's graceful shutdown, which closes a
connection it has read nothing from; the response's sequence works once hyper has read. The method
is `connections_read` rather than `accepted`, saying what it counts. The alternative is an accept
count plus a fixed wait, which F373 rules out, or a hook into hyper's own state, which hyper does
not expose.

### S2. The slot is taken inside the handler's call

Decision 3. The core's `dispatch` offers no point between the guards and the interceptors, so an
interceptor's entry runs for a connection then refused for capacity. The alternative is a hook in
`ulo::dispatch` between the guards and the rest of the stack, a core change for one transport.

### S3. A message read as the last place goes waits, unhandled, on its connection

Decision 4. Strict bounding of handlers costs at most one message read beyond the bound per
connection; a check before each read alone lets every connection waiting in a read take one more.
A waiting message is dropped unanswered if the drain begins, as an unread one would be. The
"stops reading" half of the mechanism is not observable from a test: with it removed, the bound
on handlers holds and every test passes.

### S4. The server-wide setting is `server_max_inflight`

Decision 4. The alternative is `max_inflight` for the server-wide bound, as on the other
transports, and a new name for the per-connection one, which renames the gateway attribute.

### S5. S15 is probed, not pinned

Decision 5. The widened probe shows the order matters on RabbitMQ and Kafka; nothing in the tree
fails if a later change reverses it, beyond the comment at the spot.

### S6. The hubs' adapter check stays though `connect` now makes it first

Decision 10. As the response directs. The `listen()` refusal it gave is no longer reachable
through `WsModule`, and no test pins it.

### S7. The held `cancel` goes out alone

Decision 7. The alternative is the held frames then the `cancel`, which sends a cancelled call's
items to the server only for it to discard them.

### S8. The writers' queue holds a waiting send's frame before it has room

Decision 9. A send's frame is in the queue from its call, so the queue's length past 64 is the
number of sends waiting for room, as each such send held its frame before; the writer still takes
64 ahead of any wait. The alternative is a ticket taken at the call and a reorder buffer in each
writer, which keeps the frame out of the queue until room but has to skip the ticket of a send
dropped before it pushed.

## Verification

Full, unfiltered output of every run is in the session scratchpad, `batch23/`: `runs/`,
`broken/`, `specs/`, `specs2/`, `specs3/`, `loops/` (with `summary.txt` and every run's output),
`probes/f380/`, `suites/` and `suites2/` (each with `summary.txt`), `verify/` and `verify2/`
(each with `summary.txt` and `tree.txt`) and `scripts/`. `verify/` and `suites/` ran before
F380's fix, `verify2/` and `suites2/` after it; the counts below are the later ones where both
exist.

- **The drain scenarios, 100 runs each** from copies of the built binaries (`loops/`), a run
  counted passed only on the expected "N passed; 0 failed": the WebSocket drain scenario 100 of
  100 on the standalone server and 100 of 100 on the HTTP port; `drain_http1`, both modes per run,
  100 of 100 on hyper, axum, actix, actix over HTTP/2, salvo and poem. Rocket declares it not
  applicable. The loop, run with a name matching no scenario, counted the run failed. F380's fix
  touches none of these crates.
- **F380,** from copies of the built binaries (`loops/`): `cancel_follows_its_request` 200 of 200
  on TCP and 100 of 100 on Redis; 200 of 200 on TCP under ten CPU-bound `yes` processes, where
  the old writer failed 17 of 200. Before the fix, 84 of 100 on TCP and 83 of 100 on `7cfd944b`.
- **The order scenarios on every link** after the fix (`suites2/`): `client_stream_in_order`,
  `cancel_before_opened` and `cancel_follows_its_request` passed on TCP's three suites, Redis, and
  where applicable on UDP (`client_stream_in_order` passing; it asserts UDP's refusal), NATS, MQTT,
  RabbitMQ and Kafka.
- **The TCP crate,** `cargo test -p ulo-rpc-tcp --locked --no-fail-fast`, 30 runs: 30 of 30, each
  100 passed and 1 ignored across 10 binaries.
- **Broker suites,** three runs each after the fix, one crate at a time, `--features integration
  --test conformance --locked`, test time as libtest reports it:

| Suite | Run 1 | Run 2 | Run 3 | Result |
| --- | --- | --- | --- | --- |
| `ulo-rpc-nats` | 3.41 s | 3.03 s | 3.16 s | 29 passed, 1 ignored |
| `ulo-rpc-redis` | 5.59 s | 5.59 s | 5.68 s | 31 passed |
| `ulo-rpc-mqtt` | 5.70 s | 5.66 s | 5.62 s | 34 passed, 1 ignored |
| `ulo-rpc-rabbitmq`, `ULO_CONFORMANCE_PARALLEL=6` | 19.10 s | 19.48 s | 20.00 s | 29 passed, 1 ignored |
| `ulo-rpc-kafka` | 27.80 s | 24.67 s | 24.06 s | 30 passed, 1 ignored |

  `ulo-ws-redis --test integration`, before the fix and unaffected by it: 1 passed, `peer_instance`
  ignored (0.47 s).
- **Each crate,** `cargo test -p <crate> --locked` with the OpenSSL flags, once: `ulo-ws` 52
  passed and 4 ignored (50 before: the table test replaced, one table and one hand-off test
  added); `ulo-ws-hyper` 73 and 1 (70: two limits tests and one prepare test); `ulo-ws-conformance`
  1 and 2; `ulo-ws-redis` 2 and 1; `ulo-hyper-serve` 0 and 1; `ulo-http` 20 and 9;
  `ulo-http-conformance` 0 and 2; `ulo-http-hyper` 44 and 4; `ulo-http-axum` 39 and 1;
  `ulo-http-actix` 35 and 5, and its `conformance_http2` 38; `ulo-http-salvo` 39 and 2;
  `ulo-http-poem` 38 and 1; `ulo-http-rocket` 36 and 3, `drain_http1` ignored in both modes;
  `ulo-grpc` 0 and 5; and after the fix `ulo-transport` 5 and 1 (the queue's four tests added),
  `ulo-rpc` 19 and 5, `ulo-rpc-udp` 32 and 4 (the two new scenarios declared), the broker links'
  own tests as in batch 22.
- `cargo check --workspace --all-targets --all-features` with
  `CFLAGS=-I/opt/homebrew/opt/openssl/include LDFLAGS=-L/opt/homebrew/opt/openssl/lib`, and with
  defaults: exit 0, the 17 known warnings in `crates/ulo/src` and no other location
  (`scripts/diag.py`).
- `cargo +1.88 check --workspace --all-targets --exclude ulo-http-salvo --exclude
  ulo-graphql-async-graphql`: exit 0, the same 17 and no other.
- `cargo test --workspace --no-fail-fast` with the OpenSSL flags: 665 passed, 0 failed, 77 ignored
  across 166 binaries (`verify2/workspace-test.txt`): batch 22's 653, plus two in `ulo-ws`, three
  in `ulo-ws-hyper`, five in TCP's three suites (two stamped scenarios each, the hand-written check
  gone) and the queue's four, less rocket's `drain_http1` in two modes, now ignored, as are UDP's
  two new scenarios; the 77th ignored is `Hyper::read_count`'s `ignore` doc example.
- **The tree checks,** `cargo tree -p <crate> -e normal -i tokio --locked` (`verify2/tree.txt`):
  stdout is empty for `ulo`, `ulo-transport`, `ulo-net`, `ulo-http`, `ulo-rpc`, `ulo-ws`,
  `ulo-graphql-ws`, `ulo-graphql-http`, `ulo-ws-conformance` and `ulo-smol`, each exiting 101 with
  "did not match any packages" or 0 with "nothing to print", and with `--all-features --target
  all` for `ulo-transport`, `ulo-ws`, `ulo-rpc` and `ulo-ws-conformance`; `ulo-rpc-tcp` prints
  tokio, the positive control. The queue keeps `ulo-transport` free of tokio.
- `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --lib -p <crate>` for the eighteen touched
  library crates: each exits 0.
- `cargo +1.98.1 clippy` over the same crates and the five HTTP adapters, `--all-targets --no-deps
  --locked`, the broker links' and `ulo-ws-redis`'s `integration` features: exit 0, 26 warning
  locations outside `crates/ulo/src`, two on changed lines, both `let _ =` on a `Room` in the
  queue's tests; they now drop it with `drop(..)`, and clippy over `ulo-transport` then reports
  none on a changed line, its tests passing again. `scripts/changed_lines.py`, given an existing
  location as a planted changed line, reported it (`verify/changed-lines-selftest.txt`).
- **Containers:** the user's four, seaweedfs, mailpit, postgres:18 and redis:7, untouched and the
  only ones running after every suite run and every break (`suites/`, `suites2/` and
  `broken/summary.txt` count 4 after each). Every container a test started was removed by that
  test. No permission check refused an action.
