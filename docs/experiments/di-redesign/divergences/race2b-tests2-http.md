# Divergences: race 2b, tests batch 2, the HTTP conformance suite against the six hosts

`ulo-http-conformance` had no host running it. This batch writes a `Host` for the hyper backend
and for each of the five embedding adapters, stamps the suite in both modes, and runs it. 192
scenarios run, 32 per host. Four failed on their first run: the HTTP/1.1 drain on the reference
itself (a race in the suite), `HEAD` on rocket (an adapter bug), and the HTTP/1.1 drain on actix
in both modes, which needs a decision and still fails.

Files added: `crates/ulo-http-{hyper,axum,salvo,poem,actix,rocket}/tests/conformance.rs`. Files
changed: the six crates' `Cargo.toml` (dev-dependencies `ulo-http-conformance`, with its `rocket`
feature for rocket, and tokio's `macros` and `rt-multi-thread`), `Cargo.lock`,
`crates/ulo-http-conformance/src/cases/drain.rs`, `crates/ulo-http-rocket/src/{convert.rs,
handler.rs}`.

## The signatures

No public signature changed. `ulo_http_rocket`'s crate-private `convert::response` gained a
`head: bool` parameter.

## How each host meets the contract

| Host | Nested | Fallback | `HostValue` into the host's store | `Routing` onto `ROUTING_HEADER` |
| --- | --- | --- | --- | --- |
| hyper | the suite's `HyperHost`, invoked as `http_conformance_suite!(ulo_http_conformance::HyperHost)` | same | none (the reference) | none (the reference) |
| axum | `nest_service(PREFIX, HostLayer.layer(svc))` | `fallback_service(..)` | `middleware::from_fn` around the router, into `http::Extensions` | the same middleware, from the response extensions |
| salvo | `Router::with_path("api/{**rest}").goal(handler(..))` | `{**rest}` | a hoop on the root router, into `Request::extensions_mut` | the hoop, after `call_next`, from `Response::extensions` |
| poem | `Route::nest(PREFIX, endpoint(..))` | `Route::nest("/", ..)`, which strips nothing | `EndpointExt::around`, into `Request::extensions_mut` | the same `around`, from the response extensions |
| actix | `App::service(scope(PREFIX, ..))` | `App::default_service(scope("", ..))` | `App::wrap_fn` into actix's request store; `Embedded::forward` reads `req.extensions().get::<HostValue>()` | `wrap_fn`, from actix's response extensions |
| rocket | `mount(PREFIX, routes(..))` | `mount("/", ..)` | an `AdHoc::on_request` fairing writing `local_cache` as `Option<HostValue>`; `Embedded::forward` reads it back | `rocket_fairing::RoutingFairing` |

Each host binds `127.0.0.1:0` before `start` returns, except rocket, which binds inside `launch`:
its `start` returns once an `AdHoc::on_liftoff` fairing reports `rocket.config().port`. axum is
served through `into_make_service_with_connect_info::<SocketAddr>()` with `.peer_addr(true)`.
Every host is served through its adapter's `run`, with a oneshot as the signal `stop` fires.

`ulo-http-hyper` dev-depends on `ulo-http-conformance`, which depends on `ulo-http-hyper`. Cargo
accepts the cycle for an integration test, which links the one library build both name.

## Fixes

### 1. The HTTP/1.1 drain raced the host's accept (the suite)

- **Scenario and host:** `drain`, HTTP/1.1 shape, on `HyperHost`, either mode.
- **What it saw:** 6 of 8 full runs failed, in one mode or both. Two symptoms: `the request is
  written: Os { code: 32, kind: BrokenPipe }` writing the head's final `\r\n`, and
  `read_to_close` answering a response with no status line.
- **Cause:** the scenario connects, writes half a head, and starts `close` at once. `connect`
  returns once the kernel completes the handshake, before the accept loop has taken the
  connection. When the drain begins first, ulo-hyper-serve stops accepting and closes the
  listener, and the connection still in the backlog is reset. Run alone, the two drain tests
  passed six times out of six; the race shows under the other 30 tests' load.
- **Written:** after the half head, the scenario sends `GET /hit` on a second connection and
  requires a 200 before starting `close`. A listener hands connections over in arrival order, so
  the answered second connection shows the first was accepted. With the change, five full runs
  passed out of five, and every later run of every host.

### 2. rocket refused its own `HEAD` answer (the adapter)

- **Scenario and host:** `head_answered_from_get`, rocket, both modes.
- **What it saw:** reqwest failed with `hyper::Error(IncompleteMessage)`. Over a raw socket rocket
  closed the connection having written nothing; `GET` on the same path answered 200.
- **Cause:** for a `HEAD` the app's response carries `Content-Length: 3`, the omitted body's
  length (`service.rs`'s `without_body`), and an empty body. `convert::response` copied every
  header into rocket's response, `Content-Length` included, and set no body. rocket's default
  body reports a size of `Some(0)` (`rocket-0.5.1/src/response/body.rs:94`), and
  `_send_response` appends `Content-Length` from that size beside the headers already set
  (`server.rs:157`), so hyper 0.14 was handed `content-length: 3` and `content-length: 0` and
  dropped the connection.
- **Written:** `convert::response` never copies the app's `Content-Length`; rocket writes it from
  the body it is handed. For a `HEAD` request carrying one, the body is an empty sized body of
  that length, which rocket's `strip_body` keeps as a phantom of that size. rocket now answers
  `HEAD /hit` with `content-length: 3` and no body, as the reference does.

## Needs sign-off

### S1. actix closes a half-received request when its graceful stop begins

- **Scenario and host:** `drain`, HTTP/1.1 shape, actix, both modes. Still failing.
- **What it saw:** `a request during the drain:` with an empty answer. A probe reading the
  connection after `draining()` resolved, before the head was finished, got EOF about 75 µs
  after the drain began.
- **Cause:** `Cargo.lock` resolves actix-web 4.15.0 with actix-server 2.9.8 and actix-http
  3.18.12; area E read actix-web 4.14.0 and actix-server 2.9.7. From 4.15 actix-web wires
  actix-server's graceful-shutdown signal into every HTTP/1 dispatcher
  (`actix-web-4.15.0/src/server.rs:640-650`; absent from 4.14.0). On the signal a dispatcher sets
  `DRAINING` (`actix-http-3.18.12/src/h1/dispatcher.rs:1127`); with no request in progress it then
  sets `SHUTDOWN` and closes the connection (`:574`), and `poll_request` decodes nothing more
  (`:881`). A head not yet complete is no request in progress, so the request never reaches the
  app and nothing answers the drain's 503. The adapter's `run` sends that signal through
  `ServerHandle::stop(true)` when `Handle::stopping()` resolves, as §3.8 specifies. A
  pipelined request behind one in flight is cleared the same way (`messages.clear()` at `:575`).
- **What the reference does:** hyper answers this scenario only because a connection's first
  request counts as in progress from accept (`KA::Busy` is the default, `hyper-1.11.1
  src/proto/h1/conn.rs:1035`). Read from hyper's source, not probed: a half-written second request
  on a keep-alive connection finds the connection `KA::Idle`, which `disable_keep_alive` closes
  (`:886`), as actix closes the first. The scenario's premise, that
  a half-written head keeps a connection busy through the host's graceful stop, holds on the
  hyper hosts for a connection's first request and on no request on actix ≥ 4.15.
- **Options, none made:**
  1. Declare it: an `EmbedLimits` row saying whether a request arriving during the host's
     graceful stop reaches the app, actix declaring `false`, the suite asserting the connection
     closes without an answer there and the 503 elsewhere. A public field on `ulo-http`'s
     `EmbedLimits`.
  2. actix's `run` sends `stop(true)` later than `stopping()`, so arriving requests reach the app
     and get the 503 with `Connection: close`. That needs a second signal from the embedding,
     since `stopping()` is the only one, and changes §3.8's wiring.
  3. Pin `actix-web` below 4.15. That keeps a behaviour actix has since replaced.
  4. Rewrite the HTTP/1.1 shape around a construction every host answers. No such construction
     was found: on actix a request either reaches the app before the drain, and is admitted, or
     is closed by the host.
- **Flake note:** in one full `cargo test --workspace` run the actix drain passed. The binary from
  that run fails when re-run on its own. Under load the client's `\r\n` can be decoded before the
  dispatcher polls the shutdown signal, and the request then reaches the app and gets the 503.

### S2. `ulo_http_actix::run`'s future is `!Send`

- **What forced it:** `tokio::spawn(ulo_http_actix::run(..))` fails with E0277: `HttpServer<F, I,
  S, B>` holds `PhantomData<(S, B)>`, and `S`, actix's `AppInit`, holds `Rc`s. `run` is an
  `async fn` taking the server by value, so its future holds it before the first poll. The other
  four adapters' `run` futures are `Send`, and each conformance host spawns them.
- **Written, in the test only:** the actix host builds the server inside `spawn_blocking` and
  drives `run` there with `tokio::runtime::Handle::block_on`, on the test's own runtime.
- **Proposed:** `run` as a plain `fn` that does the synchronous part (`disable_signals`,
  `shutdown_timeout`, `run()`, which yields a `Send` `Server`) before returning
  `impl Future<Output = Result<Shutdown, BoxError>> + Send`. Callers writing `run(..).await`
  are unchanged; the return type changes, so it waits for sign-off.

### S3. salvo holds its listener open through the drain window

- **Scenario and host:** `drain`, HTTP/2 shape, salvo, both modes. Passes, at a cost of 10 s.
- **What it saw:** the request sent during the drain timed out after `PATIENCE`, 10 s. The
  scenario accepts any `Err`, so the assertion held.
- **Cause:** salvo's `try_serve` stops accepting on `stop_graceful` but keeps the acceptor, and
  so the bound listener, until every connection has closed or the bound passes
  (`salvo_core-0.92.2/src/server.rs:252-360`). The held SSE stream keeps one connection open. The
  client, having received GOAWAY on that connection, opens a new one, which the kernel completes
  and salvo never serves. On hyper and axum the same request fails at once with
  `ConnectionRefused`.
- **Effect outside the suite:** during a salvo drain, a new connection hangs until the drain
  window ends rather than being refused.
- **Proposed:** salvo's `run` takes the acceptor and builds the `salvo::Server` itself, wrapping
  the acceptor so the listener closes when `stopping()` resolves. That changes `run`'s
  parameters. Separately, the suite could fail a during-drain request that ends by the client's
  own timeout, since a timeout is neither an answer nor a refusal; with that, salvo fails until
  the first change is made.

## Observed, not changed

- **rocket's close takes the whole grace while a client holds a connection.** The HTTP/2 shape
  on rocket waits 10 s in `closing.await`. Inferred from that timing rather than from rocket's
  source: rocket's graceful stop keeps an idle connection the client has not closed until
  `shutdown.grace`, which `run` sets to the app's drain window, and
  the app's `close` waits for the host. The scenario passes; it accounts for rocket's 10 s run.
- **The HTTP/2 shape observes a new connection more often than a new stream.** After GOAWAY,
  reqwest sends the during-drain request on a new connection: hyper, axum and rocket in fallback
  mode refused it at connect in the probed runs, and salvo hangs (S3). poem, and rocket nested,
  answered it 503 on the held connection. The design's "a new stream on it is then not served"
  is checked only in those last two cases; GOAWAY itself is not observed, as `race2b-E.md` 18
  states.
- **salvo answered HTTP/2 prior knowledge** on a plain listener with its `http2` feature, so the
  HTTP/2 shape runs on salvo as well as hyper, axum, poem and rocket. On actix the HTTP/2 shape has
  not run: it follows the HTTP/1.1 shape in the same test, which fails first (S1).
- **Every declared `disconnect` held:** `AtClose` on hyper, axum and salvo observed the disconnect
  during the idle period; `AtNextWrite` on poem, actix and rocket did not, and did once the stream
  wrote again. No adapter passed a scenario its limits declare unsupported.

## Verification

Against known violations, each suite was run with its own half broken:

- axum with the middleware writing neither `HostValue` nor `ROUTING_HEADER`: 12 failures, the
  `Routing` assertions in `not_found`, `method_not_allowed`, `options`, `preflight` and
  `routing_extension`, and `host_value_present_and_absent`, in both modes.
- rocket with the `forward` copy answering `None`: `forward_copy` fails in both modes.
- The suite's drain before fix 1: the failure rate above. rocket's `HEAD` before fix 2: both modes
  fail.

Three consecutive runs per host, `cargo test -p <crate> --test conformance`, test time as libtest
reports it:

| Host | Run 1 | Run 2 | Run 3 | Result |
| --- | --- | --- | --- | --- |
| hyper | 0.31 s | 0.31 s | 0.31 s | 32 passed |
| axum | 0.32 s | 0.31 s | 0.31 s | 32 passed |
| poem | 0.73 s | 0.73 s | 0.73 s | 32 passed |
| salvo | 10.02 s | 10.01 s | 10.01 s | 32 passed (S3) |
| actix | 1.03 s | 1.01 s | 1.01 s | 30 passed, `nested::drain` and `fallback::drain` failed (S1) |
| rocket | 10.03 s | 10.01 s | 10.01 s | 32 passed (rocket's grace, above) |

poem's 0.73 s is the `AtNextWrite` disconnect wait; actix's 1 s is actix-server's one-second
shutdown poll.

- `cargo check --workspace --all-targets` and `cargo +1.88 check --workspace --all-targets
  --exclude ulo-http-salvo --exclude ulo-graphql-async-graphql` pass. The only warnings are the 17
  in `crates/ulo/src` both printed before this change.
- `cargo test --workspace --no-fail-fast`: every target passed in that run, actix's conformance
  included (the race in S1's flake note), in 2 min 59 s wall time. Run again per package, actix's
  two drain tests fail.
