# Divergences: race 2b, tests batch 3, the conformance runs' follow-ups

This batch builds what the twenty-second response signed off: S1 to S3 from
`race2b-tests2-http.md`, the RPC recovery scenario over a cut connection, the HTTP/2 GOAWAY
scenario and rocket's drain behavior. It also applies one rule to both conformance suites: no
scenario passes on silence. A client timeout fails a scenario unless the timeout is the declared
answer, and a scenario that cannot apply to a host or link is reported as ignored, not passed.
Every HTTP host and both socket links pass, three runs each.

Files changed: `Cargo.toml` (workspace `h2`), `Cargo.lock`, `crates/ulo-http/src/embed.rs`,
`crates/ulo-http-actix/src/{lib.rs, run.rs}`, `crates/ulo-http-salvo/{Cargo.toml, src/lib.rs,
src/run.rs}`, `crates/ulo-http-rocket/src/lib.rs`, `crates/ulo-http-conformance/{Cargo.toml,
src/lib.rs, src/wire.rs, src/cases/{disconnect, drain, host_values, lifecycle, routing_ext,
upgrade}.rs}`, `crates/ulo-rpc-conformance/src/{lib.rs, cases/app.rs, cases/drain.rs,
cases/recovery.rs}`, the conformance tests of `ulo-http-{hyper, actix, salvo}` and
`ulo-rpc-{tcp, udp}`.

## The signatures

```rust
// ulo_http::embed
pub struct EmbedLimits { /* .. */ pub drain_pending: DrainPending, pub drain_abandoned: DrainAbandoned } // two fields new
impl EmbedLimits {
    pub const fn drain_pending(self, drain_pending: DrainPending) -> Self;       // new
    pub const fn drain_abandoned(self, drain_abandoned: DrainAbandoned) -> Self; // new
}
pub enum DrainPending { Served, Closed }     // new; `NONE` declares `Served`
pub enum DrainAbandoned { Released, Window } // new; `NONE` declares `Released`

// ulo_http_actix: was `pub async fn run<F, I, S, B>(.., signal: impl Future<Output = Signal> + Send)`
pub fn run<F, I, S, B, Sig>(app: App<Bound>, handle: &Handle, server: HttpServer<F, I, S, B>, signal: Sig)
    -> impl Future<Output = Result<Shutdown, BoxError>> + Send + use<F, I, S, B, Sig>
where /* the previous bounds */ Sig: Future<Output = Signal> + Send;

// ulo_http_salvo: the third parameter was `server: salvo::Server<A>`
pub async fn run<A: Acceptor + Send + 'static>(app: App<Bound>, handle: &Handle, acceptor: A,
    service: salvo::Service, signal: impl Future<Output = Signal> + Send) -> Result<Shutdown, BoxError>;

// ulo_rpc_conformance
trait Broker { fn client_link(&self) -> Self::Link { self.link() } /* new, defaulted */ }
conformance_suite!(Broker; not_applicable { scenario: "reason", .. }); // new form beside `conformance_suite!(Broker)`

// ulo_http_conformance
http_conformance_suite!(Host; not_applicable { scenario: "reason", .. }); // new form beside `http_conformance_suite!(Host)`
// `app()` and `app_for(..)` build the app with `.drain_timeout(4 s)`; unset it was 10 s.
```

Callers writing `ulo_http_actix::run(..).await` compile unchanged. A salvo caller passes the
acceptor it used to wrap in `salvo::Server::new`.

The HTTP scenario list changed: `forward_copy` is gone, folded into
`host_value_present_and_absent` (decision 6), and `drain` is now four scenarios, `drain_http1`,
`drain_http2`, `drain_goaway` and `drain_abandoned`. 18 scenarios per mode, 36 per host.

What each host declares and leaves out:

| Host | `drain_pending` | `drain_abandoned` | Declared not applicable |
| --- | --- | --- | --- |
| hyper (reference) | `Served` | `Released` | `routing_extension`: the reference has no host to read `Routing` |
| axum | `Served` | `Released` | none |
| salvo | `Served` | `Released` | none |
| poem | `Served` | `Released` | none |
| actix | `Closed` | `Released` | `drain_http2`, `drain_goaway`: the adapter builds actix-web without `http2` |
| rocket | `Served` | `Window` | none |
| UDP link | | | `recovery_after_disrupt`: UDP holds no connection to lose |

## Decisions

### 1. `drain_http1` finishes the head only where it is declared `Served`, after 250 ms

- **Written:** on a `Closed` host the scenario never sends the head's final `\r\n` and requires
  the host to close the connection without writing anything. On a `Served` host it waits
  `STOP_SETTLES`, 250 ms after `draining()`, then finishes the head and requires the 503 with
  `Connection: close`.
- **Why:** with actix flipped to `Served` and the `\r\n` written at once, both actix tests passed
  in 0.01 s. actix's stop reaches a connection through its server's command loop and then its
  worker, so a head finished at once reached the app as a request. This is S1's flake note,
  reproduced every time now that `run` is spawned like the other hosts. After the 250 ms wait,
  the flipped declaration failed three runs out of three. A `Closed` host is never sent the rest
  of the head, so its branch has no race in either direction.

### 2. Item 6's premise did not reproduce: rocket closes idle connections at once

- **Probed** (temporary scenario, removed), app `close` timed from `draining()`, nested mode:

| Case | hyper/axum/salvo | poem | actix | rocket |
| --- | --- | --- | --- | --- |
| HTTP/1.1 keep-alive connection idle at the drain | closed in under 1 ms | under 1 ms | under 1 ms | under 1 ms |
| HTTP/2 connection idle at the drain (`h2` client) | GOAWAY in about 20 ms, `close` under 1 ms | same | 10 s, no GOAWAY, then reset (`http2` enabled for the probe) | GOAWAY in about 20 ms, `close` under 1 ms |
| SSE response in flight at the drain, client leaves at once | axum under 1 ms on both protocols; hyper and salvo not probed | HTTP/1.1 0.7 s, at the disconnect; HTTP/2 under 1 ms | HTTP/1.1 1.0 s; HTTP/2 10 s | 10 s on both protocols; the disconnect was observed at 0.6 s |

- **What that shows:** rocket's 10 s in the old HTTP/2 shape came from the abandoned response,
  not from an idle connection. rocket observed the disconnect at its next write, 0.6 s in, and its
  stop still waited out the whole grace. Read from source, not probed further:
  `rocket-0.5.1/src/server.rs:578` awaits the full grace timer whenever anything still holds the
  server once hyper's server future has resolved. It does not wait for the hold to end.
- **Written:** what was observed is declared as `drain_abandoned: DrainAbandoned { Released,
  Window }`, rocket `Window`, everyone else `Released`. The `drain_abandoned` scenario opens an
  SSE stream, starts the drain, drops the client and times `close`. It requires under half the
  drain window where `Released` and at least the whole window where `Window`. rocket's crate doc
  records it, and says that an idle keep-alive connection closes when the drain begins.

### 3. The suite's app drains in 4 s

- **Written:** `app_for` sets `.drain_timeout(4 s)`.
- **Why:** a `Window` host takes the whole window in `drain_abandoned`, and rocket's run dropped
  from 10 s to 4 s. 4 s keeps `window / 2` above actix's one-second shutdown poll and poem's
  0.7 s disconnect, which are the slowest `Released` hosts.

### 4. The timeout rule sits in the suite's I/O primitives, plus a bound on drain requests

- **Written:** `Running::send`, `Raw::read_until` and `Raw::read_to_close` fail the scenario on
  the client's timeout, with a message saying a timeout is neither an answer nor a refusal.
  `not_a_timeout` covers the requests scenarios send themselves: lifecycle's request after
  `close` and the HTTP/2 request during the drain. Both used to accept any `Err`.
- **Why the bound:** with salvo's fix undone and the 4 s window, the request during the drain
  still passed. salvo's own stop deadline reset the waiting connection at 4 s, before the
  client's 10 s timeout, so the request ended in a late reset. `drain_http2` now also requires
  the request to end inside half the drain window. That caught the undone fix. With the window
  temporarily at 20 s, the client's timeout ended the request first, and `not_a_timeout` caught
  it.

### 5. "Not applicable" is declared in the stamping macro and reported as `#[ignore]`

- **Written:** `http_conformance_suite!(Host; not_applicable { name: "reason" })`, and the same
  form for `conformance_suite!`, stamps each named scenario `#[ignore = "not applicable:
  <reason>"]`. The macro generates a local `macro_rules!` with one arm per declared name, and
  the `$` it needs is passed in as `[$]`. A bare `$ $d` is read as the unstable `$$`. A declared
  name that is no scenario is a compile error (checked by misspelling `routing_extension`). A
  scenario run where it does not apply fails rather than returning, so a missing declaration
  fails, and `--ignored` makes a declared one fail too (checked on UDP's recovery).
- **Why the macro:** libtest cannot skip at runtime, and `macro_rules!` cannot read a trait
  item, so the declaration sits where the tests are stamped. The `Host` trait is frozen
  (DESIGN §3.8), so nothing was added to it.

### 6. `forward_copy` folded into `host_value_present_and_absent`

- **Written:** one scenario sends the same two requests on every host. `host_extensions` decides
  only which path the assertion messages name: the host's request extensions or an
  `Embedded::forward` copy.
- **Why:** the two scenarios sent the same requests, and each returned early as a pass on the
  hosts where the other applied, which is a vacuous pass on every host.

### 7. salvo's listener closes before the stop command is sent

- **Written:** `run` wraps the acceptor in a private `Closing<A>`. Its `accept` races the inner
  accept against `handle.stopping()`. When the drain begins it drops the inner acceptor, which
  closes the listener, signals a oneshot and then never accepts again. The future beside the
  server awaits that oneshot before it calls `stop_graceful(drain)`. `holdings()` answers a copy
  taken at construction.
- **Why the order:** salvo's loop selects between `accept` and its command channel. If the stop
  command won that select, salvo would drop the accept future with the acceptor still alive
  inside `try_serve` until its connections end, which is S3's bug again.

### 8. actix's `run` does the builder work and `Handle::host` before returning

- **Written:** `disable_signals`, `shutdown_timeout`, `.run()` and `handle.host(..)` all run in
  `run`'s body. The answered future holds the app, the signal and the installation's result, so
  it borrows nothing from `handle` and holds nothing `!Send`. A refused installation closes the
  app inside that future.
- **The test:** the actix host builds the server and spawns `run`'s future with `tokio::spawn`,
  as the other hosts do; the `spawn_blocking` thread is gone.

### 9. The GOAWAY scenario drives `h2` directly

- **Written:** `drain_goaway` connects with `h2::client::handshake`, opens `/endless` and reads its
  first event, then starts the drain. It polls `SendRequest::ready()` until it fails, and requires
  the failure to be `is_go_away() && is_remote()` with `Reason::NO_ERROR`. h2 reports a received
  GOAWAY as the connection's error on the next `ready()` (`h2-0.4.19
  src/proto/streams/streams.rs:804`). A host whose `ready()` keeps succeeding past the patience
  fails, and so does one that closes the connection without a GOAWAY. `h2` is a normal dependency
  of `ulo-http-conformance`, since its scenarios live in its library, and a workspace dependency
  pinned at the 0.4 line `Cargo.lock` already carried.
- **Probed against a violation:** actix with `http2` enabled and `listen_auto_h2c` failed it with
  "connection closed because of a broken pipe" at the end of its 4 s window.

### 10. The TCP client reaches the server through a relay; `Broker` gained `client_link`

- **Written:** the TCP `Broker` binds a relay listener on its own port and forwards each
  connection to the server's address with `copy_bidirectional`. One task per relayed connection
  sits in a `JoinSet`. `disrupt` aborts every task and awaits them, which closes both sockets,
  and the relay keeps accepting. `client_link()` answers `Tcp::new(relay)` and `link()`
  `Tcp::new(server)`. `client_link` defaults to `link()`, and the suite's `client()` calls it.
- **Cost:** every TCP scenario's client goes through the relay. On the drain's new call, the
  relay accepts the connection, fails its upstream connect and closes the client's connection,
  where before the client's own connect was refused. Both fail `Unavailable`, which is what the
  scenario asserts.

### 11. The recovery scenario requires the waiting call to fail `Unavailable`

- **Written:** a 3 s `HOLD` call is in flight when `disrupt` runs. It has to fail `Unavailable`
  within its own length plus `Budget::recovery`, and then a call has to succeed within
  `Budget::recovery`. `disrupt` itself is bounded by `Budget::recovery`. An unbounded `disrupt`
  that never returned hung the first violation run.
- **Probed against a violation:** an empty TCP `disrupt` failed with `got: Ok(3000)`.

### 12. The RPC drain accepts `Timeout` only from a link without `miss_signal`

- **Written:** the during-drain call's `Timeout` passes only where `!capabilities.miss_signal`,
  which is the condition `unhandled_pattern` uses. It used to pass for any link that was not
  `Addressed`.
- **Why:** the twenty-second response's rule, applied: the client's timeout passes only where it
  is the declared answer. Nothing changes on TCP or UDP, which are `Addressed`.

## Needs sign-off

### S1. `drain_abandoned` replaces item 6's idle-connection limit

Item 6 asked to declare rocket waiting out its grace for an idle connection. That did not
reproduce (decision 2). The declared field records what did: a response abandoned during the
drain makes rocket's stop take the whole window. It is a public field and enum on `EmbedLimits`
in the shape of `drain_pending`. The alternative is a note in rocket's docs with no field, and
then nothing asserts it.

### S2. salvo's `run` no longer takes a configured `salvo::Server`

`run` builds `salvo::Server::new(Closing { .. })` itself, as proposed. A caller loses
`Server::with_http_builder`, `http1_mut`, `http2_mut` and `fuse_factory`, because the server no
longer passes through its hands. The alternative keeps that configuration: make the wrapping
acceptor public (`ulo_http_salvo::closing(&handle, acceptor)`) and have `run` take
`salvo::Server<Closing<A>>`, so the type still forces the wrapper.

### S3. actix with `http2` enabled sends no GOAWAY

The adapter builds actix-web without `http2`, so its declared behavior is HTTP/1.1 only, and the
test host declares the HTTP/2 scenarios not applicable. A user whose own build enables actix's
`http2` and serves h2c gets no GOAWAY at the drain. Every HTTP/2 connection stays open until
`shutdown_timeout` and is then reset (decision 2's table and decision 9). Options: declare it,
for instance a `drain_http2` row on `EmbedLimits`; or document it in actix's crate doc; or leave
it outside the adapter's scope.

### S4. The RPC drain's tightened `Timeout` branch, for the broker links

Not run: no broker link is wired on this branch. RabbitMQ declares `miss_signal: true` and keeps a
request queued while no consumer takes it. Read from its link and AMQP's queue semantics, not
probed: a draining server's queue would leave the caller its own `Timeout`, which decision 12 now
fails. Whether that is a finding against the link or needs a capability is open until the broker
suites run.

### S5. DESIGN text these changes falsify, left for the fold

Not edited: transports DESIGN §3.8's limits table, which lacks the two rows and the adapter
table's two columns; its salvo and actix `run` bullets; and its conformance paragraph. That
paragraph lists `forward_copy` and the early return of the host-value scenario, says a single
drain scenario has two shapes, and does not mention `not_applicable` or the timeout rule. Also
§5.2's `Broker` description, which has no `client_link`, and its recovery scenario, which now
asserts the waiting call's `Unavailable`.

## Not covered

- **A broker link whose client library reconnects on its own.** The recovery scenario requires
  the waiting call to fail `Unavailable`. A link that carries the call across a reconnect, as
  NATS's client may, would fail it, and would need a declaration when its suite is wired.
- **A pipelined request behind one in flight during actix's drain.** S1 of `race2b-tests2-http.md`
  read actix as dropping it. No scenario sends one.

## Verification

Against known violations, each change temporarily undone or flipped, run, restored, and the files
confirmed byte-identical by `shasum -c`:

- actix declaring `Served`: `drain_http1` failed in both modes, three runs out of three, after
  decision 1 ("did not answer a request finished during the drain with 503: \"\"").
- axum declaring `Closed`: failed in both modes ("kept a connection whose request head was still
  arriving open through the drain").
- rocket declaring `Released`: `drain_abandoned` failed, "its stop took 4.0034s of a 4s drain
  window". axum declaring `Window` failed, "ended after 327µs".
- salvo with the listener kept open: `drain_http2` failed on the drain-window bound at the 4 s
  window, and on `not_a_timeout` at a 20 s window (decision 4).
- actix serving h2c: `drain_goaway` failed (decision 9).
- TCP with an empty `disrupt`, and UDP's recovery run with `--ignored`: both failed with `got:
  Ok(3000)`.
- A misspelled `not_applicable` name: E0425 at the declaration.

Three consecutive runs per target, `cargo test -p <crate> --test conformance` on stable, test time
as libtest reports it:

| Target | Run 1 | Run 2 | Run 3 | Result |
| --- | --- | --- | --- | --- |
| `ulo-http-hyper` | 0.32 s | 0.31 s | 0.31 s | 34 passed, 2 ignored |
| `ulo-http-axum` | 0.32 s | 0.31 s | 0.31 s | 36 passed |
| `ulo-http-salvo` | 0.32 s | 0.31 s | 0.31 s | 36 passed (was 10.0 s) |
| `ulo-http-poem` | 0.75 s | 0.73 s | 0.73 s | 36 passed |
| `ulo-http-actix` | 1.03 s | 1.01 s | 1.02 s | 32 passed, 4 ignored |
| `ulo-http-rocket` | 4.03 s | 4.02 s | 4.02 s | 36 passed (`drain_abandoned` takes the 4 s window) |
| `ulo-rpc-tcp` | 1.31 s | 1.31 s | 1.31 s | 20 passed |
| `ulo-rpc-udp` | 1.31 s | 1.31 s | 1.31 s | 19 passed, 1 ignored |

A client timeout now fails the scenario it ends (decision 4), and every run also finishes under
the 10 s patience. rocket's 4 s is `drain_abandoned` measuring the declared window. That scenario
passes when `close` completes, not on a timeout.

- `cargo check --workspace --all-targets` on stable and `cargo +1.88 check --workspace
  --all-targets --exclude ulo-http-salvo --exclude ulo-graphql-async-graphql` pass. Each prints
  the same 17 warnings, all in `crates/ulo/src`, the set listed in `batch2a-cfgattr.md`. `use<..>`
  on actix's `run` compiles on 1.88.
- `cargo test --workspace --no-fail-fast`: 270 passed, 0 failed, 61 ignored, 241 s wall time.
