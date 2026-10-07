# Divergences: race 2b, tests batch 6, the NATS drain's lost reply (F329)

`race2b-tests5-brokers.md` F3 recorded one NATS run of the conformance `drain` scenario in which
the call held across the drain received no reply and ended at its own 30 s timeout. It is an
ordering bug in the NATS link's `close`, and it reproduces on demand. The reply is published and
then lost inside async-nats: `close` asks the connection to drain and drops the last `Client`
before the connection's task has written the reply to its socket, and a task that finds its
command channel closed exits without writing what it holds. `close` now waits until the server
has routed everything the connection published, and only then drains it.

Files changed: `crates/ulo-rpc-nats/src/link.rs`.

## Reproducing it

The scratch harness is a binary crate added to a snapshot of `HEAD` (`81483c5b`, `git archive`) in
the session scratchpad, beside the workspace members so it builds against the same `Cargo.lock`.
It runs `ulo_rpc_conformance::cases::drain::drain::<NatsServer>()`, with `NatsServer` copied from
`crates/ulo-rpc-nats/tests/conformance.rs` (`nats:2.10.14`, the client through the relay), once
per iteration on a fresh multi-thread runtime. A `tracing_subscriber` writes into a buffer at
`async_nats=trace,ulo_rpc=trace,ulo_rpc_nats=trace,ulo=debug`, cleared per iteration and written to
a file when the iteration fails. `ulo_rpc` and `ulo_rpc_nats` write no event on this path, so the
evidence is async-nats's own connection trace: every `write operation` it logs is a command taken
from the client's channel and queued for the socket.

Before the fix, on a 10-core machine, Docker through OrbStack:

| Runner | Profile | Conditions | Iterations | Failed |
| --- | --- | --- | --- | --- |
| harness | release | one process | 7 | 6 |
| harness | release | three processes at once, beside three debug ones | 30 | 14 |
| harness | debug | three processes at once, beside three release ones | 30 | 19 |
| `conformance` test binary, `drain --exact` | debug | three processes at once | 120 | 4 |
| `conformance` test binary, `drain --exact` | debug | three processes at once, ten `yes` hogs | 60 | 0 |
| `conformance` test binary, whole suite | debug | three processes at once | 75 | 9 |

Every failure is the same assertion, `drain.rs:57`, "the in-flight call finishes during the drain:
RpcError { kind: Timeout, .. }". The four `drain --exact` failures were run without keeping their
output; the nine whole-suite failures and all 39 harness failures carry the message. All 39
harness traces show the sequence below, the reply queued in the batch that ends in `closed`. The
harness's trace-level formatting runs inside the tasks involved, and that is the one difference
from the test binary; the higher rate is consistent with a wider window, not shown to be caused
by it.

## The failing trace

Iteration 1 of the first harness run, the timestamps the subscriber's uptime. `sid=10` is the
server's `conformance.hold` subscription, `sid=16` its control-lane subscription, and
`_INBOX.0dXiFidV89yolgEpguzyzd` the client's inbox.

```text
30.901899250s async_nats::connection: write operation: PUB subject=conformance.hold reply=_INBOX.0dXiFidV89yolgEpguzyzd.3 payload_len=4
30.902040125s async_nats::connection: read operation: MSG subject=conformance.hold sid=10 reply=_INBOX.0dXiFidV89yolgEpguzyzd.3 payload_len=4
31.425578417s async_nats::connection: write operation: UNSUB sid=10 max=None        (the drain: one per pattern)
31.698389042s async_nats::connection: write operation: PUB subject=conformance.add reply=_INBOX.0dXiFidV89yolgEpguzyzd.4
31.699184084s async_nats::connection: read operation: HMSG subject=_INBOX.0dXiFidV89yolgEpguzyzd.4 sid=1 status=Some(503)
32.204357584s async_nats::connection: write operation: PUB subject=_INBOX.0dXiFidV89yolgEpguzyzd.3 reply=None payload_len=27
32.204362125s async_nats::connection: write operation: UNSUB sid=16 max=None
32.204363250s async_nats::connection: write operation: PING
32.204410875s async_nats: event: draining
32.204411584s async_nats: event: closed
60.904741667s f329_harness: PANIC: panicked at crates/ulo-rpc-conformance/src/cases/drain.rs:57:21:
              the in-flight call finishes during the drain: RpcError { kind: Timeout, message: "the call timed out", .. }
```

The held call reached the server 0.5 s before the drain. The refused call got its no-responders
503. At 32.204 s the server's connection task takes three commands in one batch, the `HOLD`
reply (`{"t":"res","id":3,"d":1300}`, 27 bytes), then the drain's `UNSUB` and `PING`, and reports
`draining` and `closed` 50 µs later with no socket write and no read in between. No `MSG` on
`_INBOX.0dXiFidV89yolgEpguzyzd.3` ever reaches the client; at 60.9 s the client's own timeout fires
and it publishes its `cancel`.

The same point after the fix, from a passing run of the fixed harness (`_INBOX.Hofg6..` the
client's inbox, `_INBOX.gwWP..` the round trip's):

```text
3.335114334s write operation: PUB subject=_INBOX.Hofg6BZFNDgrNn1v7tNR8l.3 payload_len=27
3.335126542s write operation: UNSUB sid=16 max=None
3.335131000s write operation: SUB subject=_INBOX.gwWPGtSDyWNro85zDSgn4U sid=17
3.335134459s write operation: PUB subject=_INBOX.gwWPGtSDyWNro85zDSgn4U payload_len=0
3.336015167s read operation: MSG subject=_INBOX.gwWPGtSDyWNro85zDSgn4U sid=17 payload_len=0
3.336074292s read operation: MSG subject=_INBOX.Hofg6BZFNDgrNn1v7tNR8l.3 sid=1 payload_len=27   (the client has the reply)
3.336168084s event: draining
3.336170000s event: closed
```

## The cause

The reply lost is the `res` of the held `conformance.hold` call. The server wrote it through its
`ReplyPath`, whose `client.publish(..)` returned `Ok`, and it was lost in the server connection's
async-nats task, after that task had taken it from the command channel and before it wrote it to
the socket.

The ordering that allows it, read against async-nats 0.46.0's `ProcessFut::poll` (`src/lib.rs`,
lines 534–655) and `Client::drain` (`src/client.rs`, line 746):

1. The call task sends the reply: `Client::publish` puts a `Command::Publish` on the connection
   task's channel and returns. Nothing is written yet.
2. The call task ends and drops its `Execution`. The core's drain sees no live execution and moves
   on to `OnModuleDestroy` and then `Server::close`, which calls `Nats::close`.
3. `Nats::close` calls `Client::drain`, which puts a `Command::Drain { sid: None }` on the same
   channel and returns. `close` then returns and drops `ServerSide`, which holds the last `Client`.
   The request lanes and the control lane, which held the other handles, ended during the drain.
4. The connection task runs. `poll_recv_many` hands it the publish and the drain together, and it
   queues both into its write buffer. Its loop polls the channel again before writing, finds it
   closed and empty, and returns `ExitReason::Closed` (`Poll::Ready(_) => return
   Poll::Ready(ExitReason::Closed)`). The write buffer is dropped with the connection.

When the connection task runs between steps 1 and 3, it writes the reply and the run passes. When
step 3 completes first, the reply is lost. `Client::drain`'s documentation says it "flushes any
remaining messages, then closes the connection"; the implementation keeps that only while some
handle outlives the drain, because the flush happens in a later poll of the same task.

Which side is wrong: transports DESIGN §10 has in-flight calls finish in the drain window and
`Link::close` close the links after it; DESIGN §9.5 step 6 lets a `close` wait on a broker's
acknowledgment under its bound. The core waited for the execution, and the dispatcher sent its
reply before the execution ended. The NATS link's `close` is the part that broke the guarantee: it
treated `publish` and `drain` returning as delivery and dropped the connection with the reply still
queued in it.

## The fix

```rust
// crates/ulo-rpc-nats/src/link.rs (private)
async fn shut(client: &Client) -> Result<(), BoxError>;
```

`Nats::close` calls `shut` on the server side's connection and on the client side's, in place of a
bare `Client::drain`. While the connection is `Connected`, `shut` subscribes to a fresh inbox on
it, publishes an empty message there, and waits for the message to come back; then it drains. NATS
routes one connection's messages in the order it sent them, so the echo coming back shows that
every message published earlier on that connection, the drained calls' replies among them, was
routed to its subscribers. While the connection is not `Connected`, `shut` skips the round trip:
async-nats writes nothing queued until it reconnects, and the close drops the connection first.

- **Alternatives:** `Client::flush` waits for async-nats's own write and flush of its buffer, not for
  the server, so the reply could still sit in the socket's send queue when the connection drops;
  whether a close then delivers it depends on the operating system and on unread input (Linux
  answers a close with unread input by a reset that discards unsent data; not probed here). The
  round trip waits for the server's routing, which is what the guarantee needs. Keeping a
  `Client` until async-nats reports `Event::Closed` does not wait for the server either: the
  drain's exit happens on the next poll of the task, whatever woke it. Patching async-nats is not this repository's to do.
- **What it costs:** one round trip to the server per connection at `close`. A connection that
  drops during the round trip leaves `shut` waiting until the core's `close` bound drops it, which
  DESIGN §9.5 step 6 records as `ShutdownFailure::Close` with `TimedOut`.
- **The client half:** at `81483c5b` `RpcClient` never calls `Link::close` (F327), and the client
  half's `shut` runs only where something closes the client's link; the batch building F327's fix in
  parallel has `RpcClientModule` close it on the app's destroy hook. The loss it guards against is
  the same: an `emit` or a `cancel` published right before the close.

## Verification

Against the known violation: the pre-fix rows above, the harness and the test binary run against
the unchanged tree.

After the fix, the same harness and the same test binary built from the snapshot with only
`crates/ulo-rpc-nats/src/link.rs` replaced by the fixed file:

| Runner | Profile | Conditions | Iterations | Failed |
| --- | --- | --- | --- | --- |
| harness | release | one process | 5 | 0 |
| harness | release | three processes at once, beside three debug ones | 150 | 0 |
| harness | debug | three processes at once, beside three release ones | 150 | 0 |
| harness | release | as above, ten `yes` hogs on the ten cores | 150 | 0 |
| harness | debug | as above, ten `yes` hogs on the ten cores | 150 | 0 |
| `conformance` test binary, whole suite | debug | three processes at once | 75 | 0 |
| `conformance` test binary, `drain --exact` | debug | three processes at once | 150 | 0 |

830 runs of the scenario after the fix, none failing, against 52 failures in 322 before it. The ten
hogs ran through the whole loaded set; the two test-binary builds overlapped part of it.
Iterations under load took 1.93–4.76 s against about 1.8 s without.

In the repository: `cargo clippy -p ulo-rpc-nats --all-targets --features integration` prints no
warning from the crate (the 17 printed are `crates/ulo/src`'s, the set `batch2a-cfgattr.md` lists),
and `cargo test -p ulo-rpc-nats --features integration --test conformance` passes three runs in a
row on the working tree, which carries the parallel batch's uncommitted changes besides this one:
20 passed each time, in 6.96 s, 5.68 s and 3.58 s.

The Docker engine stopped answering its API once during the session, after the loaded harness set
had finished; `orb restart docker` brought it back, and no row above contains a run from the
outage.

## Found by reading, not fixed

- **A request routed to a draining subscription can be dropped without an answer.** The request
  lane's `Subscriber::drain` queues `UNSUB` and `PING` and pushes the subscription onto
  `drain_pings`; on the task's next poll async-nats removes the subscription, without waiting for
  the server's `PONG` (`src/lib.rs`, lines 559–564), and a `MSG` that arrives for a removed `sid` is
  discarded (`handle_server_op`, no branch for a missing subscription). A request the server routed
  to this instance before it processed the `UNSUB`, still in flight when the subscription is
  removed, never reaches the link: the caller gets its own `Timeout` instead of the `Unavailable`
  the drain answers. Not probed; the scenario sends its refused call 250 ms after the drain starts,
  outside the window. The link cannot close the window through async-nats's public API:
  `Subscriber::unsubscribe` removes the subscription at once (`src/lib.rs`, lines 809–826). Filed as
  F331.
