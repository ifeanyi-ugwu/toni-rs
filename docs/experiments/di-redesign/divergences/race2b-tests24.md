# Divergences: race 2b, tests batch 24, `TaskEnd::Panicked(Arc<Redacted>)` with `Clone` and three helpers (S1), the app's runtime always the core's wrapper and redacting a spawned task's panic with the app's secrets (S2), the runtime suite's `warn` scenarios failing with one sentence under a test binary's own subscriber (S4), the WebSocket suite's `upgrades` declaration with embed hosts inside axum, salvo, poem, rocket and actix, closing F375 (S8), F381 found and fixed, F382 filed, and from the forty-sixth response WebSocket's "stop reading" made observable (S3), a held `cancel` bounded closing F379, and F377 recorded as hyper's behaviour

The forty-fourth response, signed off 2026-10-10, answers `race2b-tests21.md`; the forty-sixth,
signed off the same day, answers `race2b-tests23.md`, and its S3, F379 and F377 joined this batch
while it was building. `TaskEnd` is
`Clone` again, its panic carried as `Arc<Redacted>` and shared by every poll of a handle;
`is_finished`, `is_aborted` and `is_panicked` join it and `Redacted::copy_panic` is gone. The app's
runtime, `Dep<dyn Runtime>` and `AppHandle::runtime()`, is now the core's `Clocked` wrapper in
every configuration, `.runtime(r)` alone included, and a task spawned through it ends `Panicked`
with every secret the graph registered replaced; its drop-time `warn` is redacted the same way, and
a `load` refreshes the secrets. A task spawned on a runtime outside any app keeps the userinfo strip
alone. The runtime suite's two `warn` readers fail with "the test binary installed its own
subscriber, so this scenario can't capture the warning" when the binary set a global subscriber
first. The WebSocket suite's `Host` declares `upgrades()`, asserted both ways by a new scenario, and
gains a `serve` hook through which an embed host serves the app on its own listener; axum, salvo and
poem pass all 31 scenarios, rocket 29 with two declared not applicable (F378, F382), and actix runs
as a host declaring none. Running poem found F381, the suite's client reading a close-delimited body
as empty, fixed here. A WebSocket server's `MessagesRead` counts the data messages its connections
read off their sockets, and a test holds the count to the server's bound plus one message per
connection, which fails with "stop reading" removed and the handler bound kept. A broker link drops
a held `cancel` once a five-second hold runs out, and two stamped scenarios read the link's table
through a doc-hidden probe. F377 gets a note recording it as hyper's behaviour; no text in the tree
called it a defect.

Files changed: `Cargo.lock`; `crates/ulo/src/{runtime.rs, redact.rs, app/mod.rs, app/shared.rs,
app/load.rs, app/handle.rs, testing.rs}`; `crates/ulo-runtime-conformance/src/{lib.rs,
cases/mod.rs, cases/task.rs, cases/set.rs, cases/app.rs}`; `crates/ulo-tokio/{Cargo.toml,
tests/app.rs, tests/handle.rs, tests/own_subscriber.rs (new)}`; `crates/ulo-ws-conformance/src/{lib.rs,
client.rs, cases/mod.rs, cases/handshake.rs}`; `crates/ulo-ws-hyper/tests/conformance.rs`;
`crates/ulo-ws/tests/conformance.rs`; `crates/ulo-http-{axum,salvo,poem,rocket,actix}/Cargo.toml`
and `tests/ws_conformance.rs` (new) in each; `crates/ulo-http-axum/tests/websocket.rs` and
`crates/ulo-http-actix/tests/websocket.rs` deleted; for S3, `crates/ulo-ws/src/{lib.rs, table.rs,
connection.rs}`, `crates/ulo-ws-hyper/{src/lib.rs, tests/limits.rs}`; for F379,
`crates/ulo-rpc/src/{lib.rs, __private.rs, held.rs (new)}`, `crates/ulo-rpc-conformance/src/{lib.rs,
cases/order.rs}`, `src/link.rs` and `tests/conformance.rs` of the NATS, RabbitMQ, MQTT and Kafka
links, and the declarations in `tests/conformance.rs` of UDP and Redis and TCP's three suites; in
the workspace's `FRAMEWORK_GAPS.md`, notes under F375, F377 and F379, and F381 and F382 filed. The
tree is `175f090e` plus this batch; `175f090e`, committed during the build, touches
`transports/RESPONSE.md` alone, and the batch started from `ce69276b`.

## The signatures

```rust
// ulo, changed
#[derive(Clone, Debug)]                // was `Debug`; `Copy`, `PartialEq`, `Eq` and `Hash` stay off
pub enum TaskEnd {
    Finished,
    Aborted,
    Panicked(Arc<Redacted>),           // was `Panicked(Redacted)`
}
impl TaskEnd {                         // new
    pub fn is_finished(&self) -> bool;
    pub fn is_aborted(&self) -> bool;
    pub fn is_panicked(&self) -> bool;
}

// ulo_runtime_conformance::cases, new
pub const SUBSCRIBER_TAKEN: &str = "the test binary installed its own subscriber, so this scenario can't capture the warning";

// ulo_ws_conformance, changed and new
pub trait Host: Sized + 'static {
    fn upgrades() -> bool;                                             // new, required
    fn serve(&self, app: App<Bound>) -> impl Future<Output = Serving>; // new, defaulted
    /* `bind`, `connections_read`, `connect` and the rest unchanged */
}
pub struct Serving {                                                   // new
    pub addresses: Vec<BoundAddr>,
    pub until_closed: BoxFuture<'static, ()>,
}
ulo_ws_conformance::ws_conformance_suite!(SomeHost; without_upgrades); // new arm

// ulo_ws_conformance::cases::handshake, new scenarios
pub async fn as_declared<H: Host>();        // stamped `handshake_upgrades_as_the_host_declares`
pub async fn without_upgrades<H: Host>();   // what that name runs under `without_upgrades`
pub async fn http_1_0<H: Host>();           // stamped `handshake_refuses_http_1_0`, split from `malformed`

// ulo_ws, new
#[derive(Clone, Debug, Default)]
pub struct MessagesRead(/* private */);
impl MessagesRead { pub fn get(&self) -> usize; }
impl GatewayDefaults { pub fn messages_read(&self) -> MessagesRead; }

// ulo_ws_hyper, new
impl Server { pub fn messages_read(&self) -> MessagesRead; }

// ulo_rpc::__private, new, doc-hidden
pub const CANCEL_HOLD: Duration;            // five seconds
#[derive(Default)]
pub struct ClientProbe { /* private */ }
impl ClientProbe {
    pub fn attach(&self, opened: impl Fn(u64) + Send + Sync + 'static, calls: impl Fn() -> usize + Send + Sync + 'static);
    pub fn withholds(&self, id: u64) -> bool;
    pub fn withhold(&self);
    pub fn withheld(&self) -> usize;
    pub fn release(&self);
    pub fn calls(&self) -> Option<usize>;
    pub fn set_hold(&self, hold: Duration);
    pub fn hold(&self) -> Duration;
}

// ulo_rpc_nats::Nats, ulo_rpc_rabbitmq::RabbitMq, ulo_rpc_mqtt::Mqtt, ulo_rpc_kafka::Kafka, new, doc-hidden
pub fn probe(&self) -> &ClientProbe;

// ulo_rpc_conformance, new
pub trait Broker { fn probe(link: &Self::Link) -> Option<&ClientProbe>; /* defaulted to `None` */ }
pub mod cases { pub mod order {
    pub async fn late_opened<B: Broker>();      // stamped `a_late_opened_releases_the_held_cancel`
    pub async fn hold_runs_out<B: Broker>();    // stamped `a_held_cancel_is_dropped_when_its_hold_runs_out`
} }
```

`ulo_runtime_conformance::runtime_suite!` stamps fifteen scenarios where it stamped fourteen;
`ws_conformance_suite!` stamps 31 where it stamped 29; `conformance_suite!` stamps two more. No handler, module or controller signature
changes, and no macro's output changes.

What changes in meaning without a signature: `Dep<dyn Runtime>`, `Dep<dyn Timer>`,
`AppHandle::runtime()` and `AppHandle::timer()` on an app built `.runtime(r)` answer the core's
wrapper around `r`, one object, where they answered `r`; `Mounted::runtime()` follows. A task
spawned through that object that panics, or whose aborted future panics while dropped, has the
app's registered secrets replaced in the message. A handle polled again answers the same
`Arc<Redacted>`. A broker link's held `cancel` and its entry are dropped five seconds after the
`cancel`, where they lasted until the link closed.

Private: in `ulo`, `redact::Redactor` (a shared `RwLock<Arc<SecretRegistry>>` with `set` and
`panic`), `AppConfig::redactor`, `AppShared::publish`, `Clocked::new` taking the redactor,
`Launched::new` and its `redactor` field, `TaskHandle::reading`, and `runtime::redacted`; `Kept` and
`Redacted::copy_panic` are gone. In `ulo-ws-conformance`, `cases::suite_app`. In `ulo-ws`, `Tracker::new` takes the
count and the read loop adds to it. In each of the four broker links, `ClientSide::expire_cancelled`
and the `runtime` and `probe` fields it reads.

Dependencies: `ulo-tokio` takes `tracing` as a dev-dependency. `ulo-http-axum`, `ulo-http-salvo`,
`ulo-http-poem`, `ulo-http-rocket` and `ulo-http-actix` take `ulo-ws-conformance`, `ulo-tokio` and
`ulo-ws` as dev-dependencies, and actix `ulo-http` with `tokio-io`; axum drops `futures-util`,
`serde_json` and `tokio-tungstenite`, which only its deleted `tests/websocket.rs` used. The lock
gains `tracing` under `ulo-tokio` and the new edges.

## Decisions

### 1. S1: `Arc<Redacted>`, kept by the handle

- **The end kept is the `TaskEnd` itself.** `TaskHandle` holds `Option<TaskEnd>` where it held a
  `Kept` enum; the first poll wraps the recorded `Redacted` in an `Arc` and every later poll answers
  a clone of the same end, so two answers to one handle carry pointer-equal messages
  (`crates/ulo/src/runtime.rs`, `impl Future for TaskHandle`). `copy_panic` had no other caller and
  is removed.
- **The helpers carry no doc comments.** Each name says what it answers.
- **Tests moved to the helpers where they read better:** the runtime suite's task, set and app
  scenarios and `ulo-tokio`'s two test files. `value.rs` keeps `matches!` for its
  `Some(Err(TaskEnd::Finished))` patterns, where a helper reads worse through the `Result`.
- **What pins it:** `a_panic_is_panicked_and_goes_no_further` asserts `Arc::ptr_eq` on its two
  answers; `assert_panicked_with_the_message` asserts `is_panicked()` before matching; the other
  scenarios read `is_finished()` and `is_aborted()`.

### 2. S2: the app's runtime is the core's wrapper whatever the builder was given

- **The redactor decision.** At `ce69276b`, `.runtime(r)` alone bound `r` itself, so nothing of the
  app stood between a caller and `r`'s `spawn`. It now always binds `Clocked { spawner: r, timer,
  redactor }`: `.runtime(r)` with `timer` as `r` upcast, and `dyn Timer` bound as that same wrapper
  upcast, so `Dep<dyn Runtime>`, `Dep<dyn Timer>` and both `AppHandle` accessors remain one object;
  `.runtime(r).timer(t)` as before with `t` as the clock (`crates/ulo/src/app/mod.rs`,
  `AppBuilder::runtime` and `timer`). See S1.
- **How the wrapper redacts.** `Clocked::spawn` wraps the task's future in the core's own `Launched`
  carrying the redactor, hands that to `r`'s `spawn`, and makes the handle `r` answers read the inner
  wrapper's record (`TaskHandle::reading`). The inner wrapper catches the panic, redacts it with the
  app's secrets and completes, so the runtime's own wrapper around it records `Finished`, which
  nothing reads; a drop-time panic is caught and logged inside the inner wrapper, so the outer one
  never sees it. `r`'s `RuntimeTask` still decides when the task is over. See S2.
- **Where the secrets come from.** `Redactor` is created with the builder's `AppConfig`, before any
  graph exists, and the wrapper holds a clone. `AppShared::new` sets it from the graph `wire` built,
  and `AppShared::publish`, which `load` now calls for the graph it publishes and for the base graph
  it puts back on a failed connect, sets it again (`crates/ulo/src/app/shared.rs`,
  `crates/ulo/src/app/load.rs`). The registry is read when a panic is redacted, not when the task is
  spawned. See S3.
- **A bare runtime.** `TaskHandle::launch`, which a runtime calls, builds `Launched` with no
  redactor: the userinfo strip alone, as before.

### 3. S4: one sentence, and a test binary that installs its own subscriber

- `Recorder::installed()` panics with `SUBSCRIBER_TAKEN` when `set_global_default` fails, in place
  of an `expect` whose message carried the library's error (`cases/mod.rs`). The panic happens
  inside the `OnceLock`'s initialiser, which leaves it unset, so every later call fails the same way.
- `crates/ulo-tokio/tests/own_subscriber.rs` is its own test binary: it installs `tracing`'s
  `NoSubscriber` as the global default, runs `panic_while_aborted` under `catch_unwind`, and requires
  the payload to equal `SUBSCRIBER_TAKEN` exactly.
- Two scenarios now read the core's `warn`: `panic_while_aborted` and the new app scenario of
  decision 2's tests; both go through `Recorder::installed()`.

### 4. S8 and F375: `upgrades()`, `serve`, and the embed hosts

- **The declaration is a function.** `Embed::limits()` is not `const`, so an embed host answers
  `fn upgrades() -> bool { <Axum as Embed>::limits().upgrades }`, reading its adapter's declaration
  rather than copying it. The standalone server and `ulo-http-hyper`'s host answer `true`. It is
  required, so every host states it. See S4.
- **Both directions.** `handshake_upgrades_as_the_host_declares` (`handshake::as_declared`): on a
  host declaring upgrades, an upgrade to `/echo` must answer 101 and echo a message; on one declaring
  none, `listen()` must refuse the suite's app with `StartupError::Configure` whose text names
  "declares `upgrades: false`", the sentence `ulo-http`'s `check_limits` writes.
- **A host declaring none is stamped `without_upgrades`.** Under that arm the declaration scenario
  runs `handshake::without_upgrades`, which first requires `upgrades()` to answer `false`, and every
  other scenario is ignored as "not applicable: the host declares no upgrades, so it serves no
  gateway on its port". Each mismatch fails: a host declaring upgrades under that arm fails the
  requirement, and a host declaring none under the plain arm fails 30 scenarios at `listen()`.
- **`serve`.** `Served::start` binds through `H::bind`, runs `listen()`, then calls `H::serve(app)`
  and spawns the future it answers on the app's runtime. The default answers
  `App<Bound>::addresses()` and `App::serve` until a close, which is what the two reference hosts
  did through the suite before. An embed host binds its own listener there, wraps it in
  `ulo-http-conformance`'s `ReadCount` where its acceptor allows, and serves through its adapter's
  `run(app, &handle, .., pending())`; the suite's close through `AppHandle::close` ends that `run`.
  Rocket's `serve` spawns `run` itself, since rocket reports its port only once it has lifted off.
- **The hosts.** Each in its adapter's `tests/ws_conformance.rs`, `PORT = Port::Http`, the app at
  the host's root (axum `fallback_service`, salvo `{**rest}`, poem `nest("/")`, rocket `mount("/")`),
  a multi-thread tokio runtime per scenario and `Upgraded::from_tokio` over a `TcpStream`. Axum,
  salvo and poem declare `CLOSES_IDLE_AT_DRAIN`, their graceful shutdown being hyper's. Rocket
  answers `connections_read` `None` and declares the drain scenario not applicable with F378.
  Salvo's host is in salvo's own tests, so the `+1.88` check, which excludes `ulo-http-salvo`,
  excludes it, and the `msrv 1.92` job's `--all-targets` check covers it.
- **The standalone tests go.** `crates/ulo-http-axum/tests/websocket.rs` asserted a gateway
  answering inside axum, which the suite now asserts 31 ways; `crates/ulo-http-actix/tests/websocket.rs`
  asserted actix's refusal with `check_limits`'s sentence, which the declaration scenario asserts on
  actix's host. See S5.

### 5. F381: found on poem, fixed in the suite's client

The poem host failed the HTTP/1.0 case: the 400 arrived with neither `Content-Length` nor chunked
coding, and `read_body` answered such a body as empty. A probe reading to the close found the
hand-off's 46-byte reason there (`probes/f381-poem-http10-refusal.txt`). RFC 9112 §6.3 ends such a
body at the close, so `read_body` now reads it to the close (`crates/ulo-ws-conformance/src/client.rs:167`).
poem's adapter hands hyper a body of no known length, which for HTTP/1.1 is chunked and for HTTP/1.0
close-delimited; both are valid, so poem is not changed. See S7.

### 6. F382: rocket upgrades HTTP/1.0, and the case splits out

The rocket host answered an HTTP/1.0 upgrade 101. rocket 0.5 keeps no protocol version on its
`Request`, and the adapter writes HTTP/1.1 into the head and `ConnInfo`
(`crates/ulo-http-rocket/src/convert.rs:21-25`, `handler.rs:61`); no handler or fairing in rocket
0.5 sees the version. Not small to fix, so it is filed as F382. Marking all of
`handshake_refuses_a_malformed_upgrade` not applicable would have dropped its three other cases on
rocket, so the HTTP/1.0 case is its own scenario, `handshake_refuses_http_1_0`, which rocket declares
not applicable citing F382; run with `--ignored` on rocket it fails, 101 against 400
(`runs/rocket-ws-http10-ignored.txt`). See S6.

### 7. S3: the messages read off the sockets, counted

- **The count.** `MessagesRead`, an `Arc<AtomicUsize>` in `ulo-ws`, rides in `GatewayDefaults`, so
  every table a server builds from its defaults counts into the server's one; the read loop adds one
  for each Text or Binary message `poll_next` yields, before it is handled or waits for a place
  (`crates/ulo-ws/src/connection.rs`, the turn after the wait). `ulo_ws_hyper::Server::messages_read`
  hands out a clone before the server moves into the app, as `read_count` does. The hand-off's
  `WsModule` carries a count in its defaults and exposes none: no test needed it. See S9.
- **What the test asserts.** `a_saturated_server_s_connections_stop_reading_off_their_sockets`
  (`crates/ulo-ws-hyper/tests/limits.rs`): a server bounded at two places, three connections, two
  `pass` handlers holding both places from the first, then three more messages on each. The count
  must reach four, the two connections idle in a read each reading one message that waits, and
  after 200 ms be at most the bound plus one per connection; a connection opened then and sent a
  message must leave the count unchanged. Released, all twelve messages are answered and read, and
  at most two ran at once.
- **What it catches.** With the read gate's two place terms removed, keeping `try_take`, the count
  reached 11 and the test failed; with `!places.is_full()` alone removed, the late connection read
  its message and the test failed; the other server-bound tests passed under both, as batch 23
  found. See S10.

### 8. F379: a held `cancel` kept for a bounded hold

- **The call's end is the `cancel`.** A call's timeout and its drop both reach the link as the
  `cancel` `Pending::drop` sends (`crates/ulo-rpc/src/client.rs`). A unary call's entry and an
  acknowledged stream's already left the table there; only a stream cancelled before its `opened`
  stayed, as `ClientCall::Cancelled`. That entry now arms a task on the link's tokio runtime which,
  after the hold, drops the entry if it is still `Cancelled`; an `opened` before then sends the
  `cancel` alone and removes it, as before. On NATS and MQTT dropping the entry drops the gate's
  sender, so the call's pump task ends with it.
- **The hold.** `CANCEL_HOLD`, five seconds, the RPC client's default timeout, in
  `ulo_rpc::__private`; the link does not learn a call's own timeout, which no frame carries. See
  S11.
- **The probe.** Each of the four links holds an `Arc<ClientProbe>` from construction and attaches
  each client side it connects: a hook delivering an `opened` as if it arrived now (the writer's
  `Job::Opened` on RabbitMQ and Kafka, `open_gate` on NATS and MQTT) and a count of the table's
  entries. The reply router asks the probe before taking an `opened`, so a test can withhold it and
  release it late; a scenario can also shorten the hold. See S12.
- **The scenarios,** stamped into every RPC suite: `late_opened` opens a streamed request with the
  `opened` withheld, waits for the server to deliver the `open` and for the `opened` to reach the
  client, cancels, requires one entry, waits 300 ms and releases, and requires the server to receive
  the `cancel` and the table to be empty; `hold_runs_out` does the same with a 300 ms hold and
  requires the table to empty no sooner than the hold. TCP's three suites and Redis declare both not
  applicable citing U17, UDP because it carries no streamed request.

### 9. F377: a note, and no text to correct

The forty-sixth response's S1 records F377 as hyper's behaviour: a connection accepted and read
from nothing is idle, and the drain closes idle connections. A search of `crates/` and `docs/` for
F377 and for the behaviour's wording found it described as hyper closing such a connection "as
idle" in `ulo-hyper-serve`'s `ReadCount`, the two suites' `connections_read` docs and drain
scenarios, and `race2b-tests23.md`, none calling it a defect, so nothing in the tree changes; the
note goes under F377.

## The tests

- **`ulo-runtime-conformance`, fifteen scenarios** on tokio and on smol:
  - `a_task_spawned_through_the_app_redacts_its_secrets` (new, `app::redacted`): a task panicking
    with a registered secret and a URL carrying a password, spawned on the bare runtime, keeps the
    secret and loses the password; the same task spawned through `Dep<dyn Runtime>` and through
    `AppHandle::runtime()` of an app whose module registers the secret has both replaced; a task
    spawned through the app whose state panics with the secret while its aborted future drops ends
    `Aborted`, and the one `warn` logged has the secret replaced.
  - `a_panic_is_panicked_and_goes_no_further` (changed): the two answers share one `Arc`.
  - The task, set and app scenarios read the helpers; `assert_panicked_with_the_message` reads
    `is_panicked()`.
- **`crates/ulo-tokio/tests/app.rs`, fifteen tests** (fourteen before):
  `a_secret_a_loaded_module_registers_is_redacted_from_a_task_s_panic` loads a module registering a
  secret after `connect` and requires a task spawned through `AppHandle::runtime()` to answer
  "a deliberate panic carrying [redacted]".
- **`crates/ulo-tokio/tests/own_subscriber.rs`,**
  `the_warn_scenario_fails_where_the_binary_installed_its_own_subscriber`.
- **S3,** `crates/ulo-ws-hyper/tests/limits.rs`:
  `a_saturated_server_s_connections_stop_reading_off_their_sockets`, as decision 7 describes.
- **F379,** `ulo-rpc-conformance`: `a_late_opened_releases_the_held_cancel` and
  `a_held_cancel_is_dropped_when_its_hold_runs_out`, passing on NATS, RabbitMQ, MQTT and Kafka and
  declared on TCP, UDP and Redis.
- **The WebSocket suite, 31 scenarios** on seven hosts: the two reference hosts and the three
  hyper-based embed hosts pass all 31; rocket passes 29, two declared; actix passes the declaration
  scenario, 30 declared.

### Before and after

Each break went through `batch24/scripts/brk.py`, batch 23's script: it replaces each span
(asserting it occurs once), runs the target, writes every touched file and `Cargo.lock` back byte for
byte, and compares a hash over `git diff HEAD` of `crates`, `Cargo.toml`, `Cargo.lock` and `.github`
and every untracked file under `crates` and `.github`. Every restore reported `ok`, the hash
`92c2d58a9047` each time for the first 22, and the tree's hash at the time for the scope addition's
breaks (`82bd47a4b248`, `3be93386eff2`, `b885ae3ecc23`, each restored to itself). Specs are in
`batch24/specs/` and `specs2/`, full output in `batch24/broken/` with `summary.txt` and
`summary2.txt`; the first runs of the two NATS and MQTT zero-hold breaks are in `summary2.txt`, their
files overwritten by the reruns.

| Break | Target | Result |
| --- | --- | --- |
| `is_finished` answering for `Aborted` | runtime suite, tokio | four failed, `a_returned_future_is_finished` among them, "a task whose future returned ended Finished" |
| `is_aborted` answering for `Finished` | same | four failed, `abort_is_aborted_once_the_future_is_dropped` and `abort_all_aborts_every_task` among them |
| `is_panicked` answering for `Aborted` | same | the two panic scenarios failed at the `is_panicked` assertion |
| a repeated poll answering a fresh `Arc` over a copied message | same | `a_panic_is_panicked_and_goes_no_further` failed, "polled again, the handle answered a copy of the message" |
| `Clocked::spawn` handing the future to the runtime unwrapped | runtime suite, tokio and smol | the redaction scenario failed on each, the secret in the text spawned through `Dep<dyn Runtime>` |
| `.runtime(r)` binding `r` itself, as at `ce69276b` | runtime suite, tokio; `ulo-tokio --test app` | the redaction scenario failed the same way; the load test failed, the loaded secret in the text |
| the redactor not set from the wired graph | runtime suite, tokio | the redaction scenario failed the same way |
| `publish` leaving the redactor | `ulo-tokio --test app` | the load test failed, "a deliberate panic carrying loaded-token-91c2" |
| an unfinished app task's future dropped with no redactor | runtime suite, tokio and smol | the redaction scenario failed on each at the drop-time `warn`, the secret in it |
| the recorder's old `expect` | `ulo-tokio --test own_subscriber` | failed, the payload "another global `tracing` subscriber was installed before the suite's: SetGlobalDefaultError(..)" |
| the recorder ignoring the failure | same | failed, the payload "logged at `warn` 0 times", the scenario having run without its recorder |
| axum's `take_upgrade` taking nothing | `ulo-http-axum --test ws_conformance` | 28 of 31 failed, the declaration scenario and every upgrade answered 400 "this request cannot be upgraded to a WebSocket" |
| salvo's handler setting no upgrade | `ulo-http-salvo --test ws_conformance` | 28 of 31 failed the same way |
| poem's endpoint setting no upgrade | `ulo-http-poem --test ws_conformance` | 28 of 31 failed the same way |
| rocket's handler setting no upgrade | `ulo-http-rocket --test ws_conformance` | 27 of 29 failed the same way |
| actix declaring `upgrades(true)` | `ulo-http-actix --test ws_conformance` | the declaration scenario failed, "the host is stamped `without_upgrades` and declares upgrades" |
| `check_limits`'s `upgrades` refusal removed | same | failed, "the host declares no upgrades, and the app listened with gateways on its port" |
| axum stamped `without_upgrades` | `ulo-http-axum --test ws_conformance` | failed the same way as actix declaring upgrades |
| actix stamped with every scenario | `ulo-http-actix --test ws_conformance` | 30 failed, "the suite's app did not listen"; the declaration scenario passed |
| axum declaring `CLOSES_IDLE_AT_DRAIN = false` | axum's drain scenario | failed, the idle connection closed where a 503 was due |
| the default `serve` answering no address | `ulo-ws-hyper --test conformance`, `ulo-ws --test conformance` | all 31 failed on each, "the host serves the suite's app on no address" |
| `read_body` answering a close-delimited body as empty, as at `ce69276b` | `ulo-http-poem --test ws_conformance` | `handshake_refuses_http_1_0` failed, "HTTP/1.0: the refusal carries no reason" |
| the read gate's place terms removed, `try_take` kept | `ulo-ws-hyper --test limits` | the stop-reading test failed, "11 messages read off the sockets of a server saturated at 2 places, over 3 connections"; the other six passed |
| `!places.is_full()` alone removed from the read gate | same | the stop-reading test failed, "a connection opened while every place was taken read a message" |
| the count never added to | same | the stop-reading test failed, "4 messages read did not happen within 5s" |
| each broker link's expiry not armed | the two held-cancel scenarios on NATS, RabbitMQ, MQTT and Kafka | `hold_runs_out` failed on each, "the held cancel's entry dropped did not happen within 5s"; `late_opened` passed |
| each broker link's expiry dropping the entry at once | same | on RabbitMQ and Kafka both failed, the entry gone before the scenario counted it; on NATS and MQTT `hold_runs_out` failed, "dropped before its hold ran out", and `late_opened`, then releasing at once, passed, the expiry task not having run; with the release made 300 ms late, both failed on NATS and MQTT, "the held cancel did not reach the server" |
| the restored tree | every target above | every test passed |

## Left for the transports DESIGN fold

Line numbers are those read on `ce69276b`.

- §1's crate table: `fw-runtime-conformance`'s row (line 46), fifteen scenarios;
  `fw-ws-conformance`'s row (line 47), the `Host` trait gains `upgrades` and `serve`, 31 scenarios,
  and the hand-off runs the list inside every embed adapter as well as on `fw-http-hyper`.
- §4.1's conformance paragraph (line 758): `fn upgrades() -> bool`, required, an embed host reading
  its adapter's `EmbedLimits`; `fn serve(&self, app: App<Bound>) -> Serving`, defaulted to the
  addresses `listen()` reported and `App::serve`, overridden by an embed host to serve through its
  adapter's `run`; `connect` takes the addresses `serve` answered; the `without_upgrades` arm; a
  refusal body with neither length nor chunks read to the close (F381).
- §4.1, the scenario list (line 762): thirty-one scenarios, handshake ten; the declaration scenario
  in both directions; HTTP/1.0 its own scenario, the malformed scenario keeping three cases.
- §4.1, the hosts (line 766): seven hosts, the two reference hosts, axum, salvo and poem passing all
  31, rocket 29 with the drain (F378) and HTTP/1.0 (F382) scenarios not applicable, actix declaring
  no upgrades; the F375 sentence and the two adapter tests go.
- §8, the `TaskEnd` block (line 1197): `#[derive(Clone, Debug)] pub enum TaskEnd { Finished,
  Aborted, Panicked(Arc<Redacted>) }` with `is_finished`, `is_aborted` and `is_panicked`; the comment
  about `matches!` goes.
- §8, the paragraph after it (line 1231): a task spawned through the app's runtime is redacted with
  the graph's secrets by the wrapper's inner `Launched`, the handle reading that record; a bare
  runtime's task keeps the userinfo strip; a handle answers the same `Arc` on every poll, and
  `copy_panic` is gone.
- §8, "The app" (line 1233): `.runtime(r)` binds `dyn Runtime` as the core's `Clocked` around `r` in
  every configuration and `dyn Timer` as that wrapper upcast; the wrapper carries the app's redactor,
  set at `wire` and on every `load`.
- §8, the runtime suite's conformance paragraph: fifteen scenarios; the redaction scenario, its
  drop-time `warn` included; `SUBSCRIBER_TAKEN` for a test binary that installed its own subscriber.
- Decision 63 (line 1496): the host is `bind`, `serve` and `connect`, with `upgrades` declared.
- Decision 76 (line 1509) is superseded on two points: `Panicked(Arc<Redacted>)` keeps `Clone`,
  without `Copy`, `PartialEq`, `Eq` and `Hash`; the registry reaches an app-spawned task through the
  app's wrapper, which `.runtime(r)` alone now binds too.
- Decision 77 (line 1510) is superseded: the WebSocket suite asserts `upgrades` both ways on every
  embed host, and the two adapter tests are gone.

- §4.1's load shedding (line 812): `MessagesRead` and `Server::messages_read`; under
  `server_max_inflight` the count rises by at most one message per connection once every place is
  taken, which a test asserts.
- §5.2's broker paragraph (line 896): a held `cancel` is dropped with its entry once `CANCEL_HOLD`,
  five seconds, runs out (F379); the conformance paragraph and scenario list (lines 908-912): the
  two held-cancel scenarios and `Broker::probe`.
- §3.7 or F377's own line: F377 is hyper's behaviour, not a defect.

And in the core DESIGN, `docs/experiments/di-redesign/DESIGN.md`:

- §3.9 (line 386): "with no registered secret to replace since a task is spawned outside any graph"
  becomes: a task spawned through the app's runtime has every registered secret replaced, its
  drop-time `warn` too; one spawned on a runtime outside any app has the userinfo strip alone.
- §3.9 (line 388): "`dyn Timer` is then the same object, upcast" holds with the object being the
  core's wrapper around `r`, not `r`.
- §9.3 (line 861): the app's runtime redacts with the registry the graph holds, refreshed on `load`.
- §9.4 (line 893): "`.timer(t).runtime(r)` is `r` for both" becomes "spawns and times on `r`",
  through the wrapper.
- Decision 24 (line 1412): "with an empty registry since `Spawn::spawn` sees no graph" becomes the
  app's registry through its runtime's wrapper, the empty one for a bare runtime.

## Needs sign-off

Numbered from S1 for this batch; the items it builds are named by their logs.

### S1. `.runtime(r)` alone binds the core's wrapper, not `r`

Decision 2. Without a wrapper nothing of the app is between a caller and `r`'s `spawn`, so the
response's S2 needs one in every configuration. `Dep<dyn Runtime>` and `Dep<dyn Timer>` stay one
object; they are no longer the object given, and a spawn through the app pays a second
`CatchUnwind` and record. The alternative is the redaction only after `.runtime(r).timer(t)`, which
leaves the common configuration unredacted.

### S2. The wrapper redacts through an inner `Launched` whose record the handle reads

Decision 2. It needs nothing of a runtime adapter beyond what `TaskHandle::launch` already asks. The
alternative is a thread-local the wrapper sets around `r.spawn(..)` and `launch` reads, one wrapper
layer instead of two, which holds only while a runtime calls `launch` synchronously on the calling
thread inside `spawn`, an obligation the SPI states and nothing checks.

### S3. The redactor follows the graph, `load` included

Decision 2. A secret a lazily loaded module registers is replaced in a task spawned after the load,
which a test pins, and, since the registry is read at the panic, in one spawned before the load that
panics after it, which is read and not tested; a failed load puts the base graph's back. The
alternative is the registry as `wire` left it.

### S4. `upgrades` is a required function, and a host declaring none uses its own arm

Decision 4. A function lets an embed host read `Embed::limits()`, which is not `const`. The
`without_upgrades` arm duplicates the declaration, and the scenario it runs requires the two to
agree. The alternative is a `const UPGRADES` that each embed host copies, and a host declaring none
listing 30 scenarios as not applicable by hand.

### S5. axum's and actix's standalone WebSocket tests are removed

Decision 4. The suite covers both: axum's gateway answering in 31 scenarios, actix's refusal in the
declaration scenario with `check_limits`'s sentence. The alternative is keeping both beside the
suite.

### S6. HTTP/1.0 is its own scenario

Decision 6. Rocket then declares one case not applicable rather than four. The alternative is
declaring the malformed scenario whole, or a per-case declaration the suite has no form for.

### S7. F381 is fixed in the suite, and poem keeps its close-delimited HTTP/1.0 answer

Decision 5. Close-delimited is valid HTTP/1.x. The alternative is poem's adapter passing the body's
length to hyper, which would hide the client's gap on poem alone.

### S8. The drop-time `warn` of an app task is pinned in the runtime suite

The redaction scenario installs the global recorder and reads the `warn` a drop-time panic of a task
spawned through the app logs. The alternative is leaving that path untested; the break that drops
the redactor there fails only this assertion.

### S9. The messages-read count rides in `GatewayDefaults`

Decision 7. Every table built from a server's defaults counts into one value the server can hand
out before it moves into the app. The alternative is a new parameter to `GatewayTable::own_port`,
or a count the table exposes, which a test cannot reach once the server is inside the app.

### S10. The test also holds a connection opened while saturated to reading nothing

Decision 7. The response's bound, plus one per connection, is met by the read gate's `waiting` term
alone; the late connection is what makes `!places.is_full()` observable too. The alternative is the
bound alone, under which removing `is_full` passes.

### S11. The hold is a constant five seconds, not the call's own timeout

Decision 8. No frame carries a call's timeout to the link. The alternatives are a knob on each
link, or a deadline carried on `open`, a wire change on every link.

### S12. A doc-hidden probe on each broker link

Decision 8. The scenarios need the client's table and a late `opened` on every broker alike; the
probe withholds the server's real `opened` on the client and delivers it later. The alternative is
a late `opened` made by starting the server's consumer after the `open`, which NATS, holding nothing
for an absent subscriber, cannot produce.

## Verification

Full, unfiltered output of every run is in the session scratchpad, `batch24/`: `runs/` for the runs
during the build, `probes/` for F381's and F382's, `broken/`, `specs/` and `specs2/` for the breaks,
`loops/s3/` for the stop-reading loop, `verify/` for a first pass before the scope addition, `verify2/`
for the pass below on the final tree, and `scripts/`.

- **The listed crates,** `cargo test -p <crate> --locked` with the OpenSSL flags
  (`verify2/summary.txt`, `verify2/crate-*.txt`): `ulo` 1 passed, 5 ignored; `ulo-transport` 5 and
  1; `ulo-tokio` 37 and 1 (the runtime suite 15, `app.rs` 15, `handle.rs` 6, `own_subscriber.rs`
  1); `ulo-smol` 15 and 1; `ulo-runtime-conformance` 0 and 1; `ulo-ws` 54 and 4; `ulo-ws-hyper` 76
  and 1 (the suite 31, `limits.rs` 7); `ulo-ws-conformance` 1 and 3, the third the
  `without_upgrades` doc example; `ulo-http` 20 and 9, with `tokio-io` 22 and 9; `ulo-http-hyper` 44
  and 4; `ulo-http-conformance` 0 and 2; `ulo-http-axum` 69 and 1 (the HTTP suite 38, the WebSocket
  suite 31); `ulo-http-salvo` 70 and 2; `ulo-http-poem` 69 and 1; `ulo-http-rocket` 65 and 5;
  `ulo-http-actix` 35 and 35 (the WebSocket suite's declaration scenario passing, 30 declared), and
  `conformance_http2` 38; `ulo-rpc` 19 and 5; `ulo-rpc-conformance` 2 and 2; `ulo-rpc-tcp` 100 and 7
  over 10 binaries, the two held-cancel scenarios declared in each of its three suites;
  `ulo-rpc-udp` 32 and 6; the four broker links' and Redis's own tests without `integration` as
  before. Every run exited 0 and reported a nonzero count where the crate has tests.
- **The WebSocket suite, three runs per host** (`verify2/ws-*`): 31 passed on the standalone
  server, the HTTP port on `ulo-http-hyper`, axum, salvo and poem; 29 passed and 2 ignored on
  rocket; 1 passed and 30 ignored on actix; the same in every run. `verify/` holds three more runs
  of each from before the scope addition, with the same counts.
- **The stop-reading test, 50 runs** from a copy of the built binary (`loops/s3/`): 50 of 50, a run
  counted only on "1 passed; 0 failed"; the loop given a name matching no test counted the run
  failed.
- **Broker suites, three runs each,** one crate at a time, `--features integration --test
  conformance --locked`, libtest's time: NATS 31 passed and 1 ignored (6.78 s, 4.27 s, 3.75 s); MQTT
  36 and 1 (6.33 s, 6.66 s, 6.29 s); RabbitMQ with `ULO_CONFORMANCE_PARALLEL=6` 31 and 1 (30.99 s,
  32.18 s, 29.64 s); Kafka 32 and 1 (33.70 s, 29.90 s, 28.26 s). Redis once, its file's
  declarations having changed: 31 passed and 2 ignored (5.88 s).
- `cargo check --workspace --all-targets --all-features` with
  `CFLAGS=-I/opt/homebrew/opt/openssl/include LDFLAGS=-L/opt/homebrew/opt/openssl/lib`, and with
  defaults: exit 0, the 17 known warnings in `crates/ulo/src` and no other location
  (`scripts/diag.py`).
- `cargo +1.88 check --workspace --all-targets --exclude ulo-http-salvo --exclude
  ulo-graphql-async-graphql`: exit 0, the same 17 and no other; salvo's WebSocket host is in salvo's
  own tests and so outside it.
- `cargo test --workspace --no-fail-fast` with the OpenSSL flags: 795 passed, 0 failed, 118 ignored
  across 170 binaries (`verify2/workspace-test.txt`). Batch 23's 665, plus two runtime scenarios
  over two runtimes, one in `app.rs`, `own_subscriber.rs`, two scenarios on each reference WebSocket
  host, 30 on axum (31 less its deleted `websocket.rs`), 31 on salvo, 31 on poem, 29 on rocket,
  none net on actix (its deleted test for the declaration scenario) and the stop-reading test, 130
  in all; ignored rises by rocket's 2, actix's 30, the `ignore` doc example, and the held-cancel
  scenarios declared in TCP's three suites and UDP's, 41 in all. Six binaries are added and two
  deleted.
- **The tree checks,** `cargo tree -p <crate> -e normal -i tokio --locked` (`verify2/tree.txt`):
  stdout is empty for `ulo`, `ulo-transport`, `ulo-net`, `ulo-http`, `ulo-rpc`, `ulo-ws`,
  `ulo-graphql-ws`, `ulo-graphql-http`, `ulo-ws-conformance`, `ulo-runtime-conformance` and
  `ulo-smol`, each exiting 101 with "did not match any packages" or 0 with "nothing to print";
  `ulo-ws-hyper` prints tokio, the positive control. `MessagesRead` and `ClientProbe` keep `ulo-ws`
  and `ulo-rpc` free of tokio.
- `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --lib -p <crate>` for the eighteen touched library
  crates: each exits 0. `ulo-rpc-conformance` failed first, the scenario docs linking the private
  `SHORT_HOLD`; the docs now write 300 ms, and the rerun exits 0 (`verify2/doc-ulo-rpc-conformance-2.txt`).
- `cargo +1.98.1 clippy` over the same crates, TCP, UDP and Redis, `--all-targets --no-deps
  --locked`, the broker links' `integration` features: exit 0, 16 warning locations outside
  `crates/ulo/src`, none on a changed line; `scripts/changed_lines.py`, given an existing location as
  a planted changed line, reported it.
- **Containers:** the user's four, seaweedfs, mailpit, postgres:18 and redis:7, before and after
  every pass, break and broker run, and the only ones running after each (`verify2/docker-*.txt`,
  `broken/summary2.txt`, `verify2/summary.txt`). Every container a suite started was removed by it.
  No permission check refused an action.
- **The stop during the build.** After a reported stop following the first 22 breaks' restores, every one had reported `ok` with its hash, each spec's span was found once in the tree
  afterwards, and `cargo check --workspace --all-targets --all-features` exited 0 before the work
  went on.
