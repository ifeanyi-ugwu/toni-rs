# Divergences: race 2b, tests batch 16, the MQTT client's `close` and the drain's end (F349, F350)

Batch 15 filed two defects in `crates/ulo-rpc-mqtt/src/link.rs` by reading. F349: the client
side's `close` queued a DISCONNECT and aborted its event loop at once, so a publish queued before it
could be lost. F350: the drain's watcher could end the inbound stream with a call held, so frames
for that call were dropped. Both are fixed in the link, each with a test in the MQTT crate that
fails against the unfixed code and passes against the fix. F350's effect differs from the filed
one: the frame it loses is the `cancel` of a call admitted before the drain, and the call the entry
describes cannot be admitted (below).

Files changed: `crates/ulo-rpc-mqtt/src/link.rs`, `crates/ulo-rpc-mqtt/tests/conformance.rs`.
`DESIGN.md` is unchanged: its MQTT text describes the server side's `close`, which still holds, and
says nothing of the client side's; its drain text, the inbound stream ending once the draining
server holds no call, is what the link now does.

## The tests

Both drive the link through `Link` directly, beside the conformance suite in
`crates/ulo-rpc-mqtt/tests/conformance.rs`, against a Mosquitto container started by that file's
`Mosquitto` and removed when the test returns or panics. A server link listens on one pattern with
its group set, and a client link connects to the same broker.

- **`client_close_writes_what_was_queued_before_it`**, on a current-thread runtime. The client's
  send of an `evt` is polled once, which queues the PUBLISH in rumqttc's request channel and leaves
  the send waiting for the packet id; the client link's `close` follows with nothing in between
  that lets the event loop run. The server's inbound stream must yield the event within 5 s.
- **`drain_delivers_the_cancel_that_releases_the_last_call`**, on a multi-thread runtime. The client
  sends a `req`, the server's inbound stream yields it, the server link's `drain` runs, and the
  client sends `cancel` for the call. The inbound stream must yield that `cancel` within 5 s.

The second test reaches F350's window only under a probe:
`tokio::task::block_in_place(|| std::thread::sleep(Duration::from_millis(50)))` in `on_publish`,
after a control message's call is looked up or released and before its frame is sent.
`block_in_place` hands the worker's queue to another thread, so the drain's watcher, woken by the
release on the same worker, can run during the sleep; a plain `std::thread::sleep` would leave it
in the worker's LIFO slot, which no other worker steals from.

## F349: the client side's `close`

### The defect

`Mqtt::close` at `8b9d451a` ended the client side with `client.disconnect().await` and
`task.abort()` (`link.rs:251-254`). rumqttc's `publish` and `disconnect` only queue requests, which
the event loop writes in order. The abort came before the loop had run again, so whatever the loop
had not yet taken from the channel was lost: an `evt`, a `cancel` (which `RpcClient` sends from a
task it spawns when a call is dropped unsettled), or any other publish. A send of a request or an
event returns only once the loop reports its `Outgoing::Publish`, after rumqttc has written and
flushed the packet, so an awaited send was never exposed; a send still in flight when `close` ran
was.

### The fix

The server side's mechanism from batch 15 (F348) is now one function, `shut` (`link.rs:262`), which
both sides call: it queues the DISCONNECT and awaits the event loop under a guard that aborts the
loop if the core's `close` bound drops the wait. The client side gains a `closed` flag, set by
`close` before it queues the DISCONNECT; `client_loop` returns on `Outgoing::Disconnect` once the
flag is set (`link.rs:863`), and a connection failing after `close` ends the loop logged at
`debug`, as on the server side. The client loop never reconnected, so that half needed no change.
`State` keeps the client side and its `JoinHandle` in place of the `AsyncClient` and an
`AbortHandle`.

On the DISCONNECT path the loop also fails the publish still waiting for its packet id, through
`ClientSide::ended`, which the connection-failure path already did inline: a publish waiting there
was queued after the DISCONNECT, rumqttc never writes it, and its send would otherwise wait on a
oneshot whose sender the link keeps, holding the publish lock until the caller's own timeout. Its
error reads "the MQTT link's connection ended before the publish went out", covering both paths.

### Runs

| Code | Runs | Result |
| --- | --- | --- |
| `8b9d451a`, unfixed | 1 | failed: "the event queued before the client's close did not reach the server, got: None" |
| fixed | 1 | passed |
| fixed, the client's `shut(&side.client, task).await` rewritten to `let _ = side.client.disconnect().await; task.abort();` | 1 | failed, the same message |

The failure is deterministic on a current-thread runtime and needs no probe. The rewritten line was
written back afterwards.

## F350: the drain's end

### The contract

Transports DESIGN §5.3, *Shutdown*: "The control lane stays open and the inbound stream ends once
the draining server holds no call, since a held call can still receive its items or a `cancel`."
`Link::drain`'s doc says the inbound stream ends "once nothing more will arrive". The NATS link
meets it by construction: its control lane delivers each control frame and decides to end, once no
call is held, on the same task (`crates/ulo-rpc-nats/src/link.rs`, `control_lane`), so a `cancel`
that releases the last call is sent before the lane can stop. The contract is unambiguous and MQTT
can meet it.

### The defect

At `8b9d451a` the MQTT watcher waited for the call table to empty and then took the inbound sender
(`link.rs:224-229`), while `on_publish` ran on the event loop and took the sender's lock only inside
`deliver`. Two orderings put a take between a call's change in the table and its delivery:

- **A `cancel` releasing the last held call.** `on_publish` released the call (`link.rs:389`), the
  count fell to zero and woke the watcher, and only then delivered the `cancel` (`link.rs:393`). A
  take between the two dropped the `cancel` of a call the server admitted before the drain. The
  handler is not cancelled and runs until it returns or the drain window's end cancels it with
  `Drain`, holding the drain open meanwhile. The caller has already stopped waiting, so what shows
  is server-side: work for a call nobody waits on, and a drain that lasts longer.
- **A request held after the watcher saw the table empty.** This is the ordering the entry
  describes. The call is delivered and then the stream ends with it held. Nothing is lost by it:
  the core advances the phase to Draining before it calls any server's `drain`
  (`crates/ulo/src/lifecycle/shutdown.rs:233-237`), `Execution::open` refuses an ordinary execution
  from Draining on (`crates/ulo/src/lifecycle/phase.rs:58-62`), and the dispatcher answers such a
  call `err` of kind `unavailable`, "the server is draining", registering nothing
  (`crates/ulo-rpc/src/dispatch.rs:177-181`). Its later `in`, `in_end` or `cancel` would find no
  registered call even if delivered. The entry's caller-visible effect, a streamed request stalling
  until its deadline, does not occur: that caller is told `opened` and then refused.

### The fix

A call is held or released, and its frame delivered, under the inbound sender's lock, and the drain
takes the sender under that lock only after finding no call held:

- `on_publish` takes the `deliveries` lock once, before it looks at the message, and holds it
  through `hold`, `get` or `release` and the send (`link.rs:387`). A `req` whose send fails, the
  core having dropped the receiver, is released and refused as before; a `req` or `open` arriving
  once the stream has ended is refused without being held.
- The watcher waits for the count to reach zero and calls `ServerSide::end_if_idle`
  (`link.rs:362`), which takes the sender only if the table is empty under the lock, and otherwise
  waits again.
- `refuse` no longer releases a call: nothing is held when it runs, and a release by key could have
  removed another call held under the same Correlation Data, such as the original of a QoS 1
  duplicate.
- The server side's `close` takes the sender before clearing the table, so no call can be held
  between the two.

The lock order is `deliveries` then the call table, the same in `on_publish`, `end_if_idle` and
`close`; a reply path releasing its call takes the table alone.

### Runs

| Code | Probe | Runs | Result |
| --- | --- | --- | --- |
| `8b9d451a`, unfixed | none | 1 | passed |
| `8b9d451a`, unfixed | 50 ms in `block_in_place` | 3 | 3 failed: "the cancel of the call the draining server held was not delivered", `left: None`, `right: Some(Cancel { id: 1 })` |
| fixed | 50 ms in `block_in_place` | 3 | 3 passed |
| fixed, the `cancel`'s send rewritten to release the lock before the probe and take it again to send | 50 ms in `block_in_place` | 3 | 3 failed, the same message |
| as committed | 50 ms in `block_in_place` | 3 | 3 passed |

The rewrite in the fourth row restores the unfixed ordering for a control frame, the release
outside the lock the send takes: `drop(deliveries)` before the probe and
`if let Some(sender) = lock(&self.deliveries).as_ref() { .. }` for the send, two lines written back
afterwards. The fixed rows before the last ran before a `req` whose send fails was again released
and refused, a change outside the control path; the last row is the committed code with the probe
added back. The probe is removed. The second ordering above has no test: the scenario reaches it
only with a request routed to the server before the broker processed the drain's UNSUBSCRIBE, and
its effect is nil.

## Verification

Full output of every run is kept in the session scratchpad, unfiltered.

- `cargo test -p ulo-rpc-mqtt --features integration --test conformance --locked`, three runs in a
  row: 26 passed each time, the suite's 24 and the two tests above, in 5.62 s, 5.33 s and 5.40 s.
- `cargo check --workspace --all-targets --all-features` with
  `CFLAGS=-I/opt/homebrew/opt/openssl/include LDFLAGS=-L/opt/homebrew/opt/openssl/lib`: the 17
  warnings in `crates/ulo/src`, no others.
- `cargo test --workspace --no-fail-fast` with the same flags: 430 passed, 0 failed, 64 ignored, batch 15's
  count. The two new tests are behind the `integration` feature and do not run here.
- `cargo +1.98.1 clippy -p ulo-rpc-mqtt --all-targets --no-deps --locked`, and again with
  `--features integration` to cover the tests: one warning from the crate in each, batch 15's
  `clone_on_copy` on the SUBACK reason code in `server_loop` (`link.rs:526`), a line this batch did
  not change. Nothing new.

## Found by reading, not fixed

- **MQTT's drain returns before the broker has processed its UNSUBSCRIBEs** (F351). With no call
  held, the inbound stream ends at once and the core's drain window closes as soon as `drain`
  returns, so `close` can run while the broker still routes requests to the instance. A request
  read after `close` queued its DISCONNECT is refused behind it or never read, and its caller,
  told `Success` by the broker's PUBACK, sees its own `Timeout`. Unprobed.
- **The RPC server's `close` drops deliveries the serve loop has not accepted and aborts refusals
  not yet sent** (F352). The core's drain window waits for live executions and for each link's
  `drain` future, not for the inbound stream to end or for the refusal tasks it spawned for calls
  arriving during the drain; `serve` then aborts those tasks and stops reading at the close signal.
  Unprobed, on every link.

## Containers

Every test started its own `eclipse-mosquitto:2.0.18` container and removed it on return or panic;
the engine's mosquitto count was ten before the first run and ten after each. The ten are the ones
batch 15's harness left (named in `race2b-tests15-mqtt.md`), still running and not touched here.
