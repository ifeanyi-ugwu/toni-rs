# Review of the fifth response: backends and embedding

Reviewed: `transports/RESPONSE.md`, "Fifth response: T2, backends and embedding", against the core
and transport designs, the code on `experiment/di-redesign` at `f210f972`, and the host crates'
sources in the local registry. The direction is taken as given; every item below is a refinement
of the answer or a question the author decides.

Each claim is marked: **Probed** (compiled or run, see the appendix), **Read** (source read, cited
`file:line`), **Spec**, or **Unverified**. Built-code paths are under `crates/`; host paths are
under `~/.cargo/registry/src/*/`, at the versions the probe crate resolved: axum 0.8.9, salvo
0.92.2 (`salvo_core` and `salvo_extra` 0.92.2), poem 3.1.12 (read only, see the appendix),
actix-web 4.15.0 with actix-http 3.18.12 and actix-server 2.9.7, rocket 0.5.1 with rocket_http
0.5.1 (http 0.2.12, hyper 0.14.32), hyper 1.11.1, tower 0.5.3.

## What holds

- A `Server` that binds nothing passes the SPI as built. `prepare` has a default, `bind` is a
  required method a no-op satisfies, and `bound()` defaults to an empty list, documented for "a
  transport with no socket of its own" (Read, `crates/ulo/src/transport/server.rs:24-69`).
  `listen()` runs every `prepare` before any `bind` and refuses an app without a `Timer` first
  (Read, `crates/ulo/src/app/mod.rs:199-236`).
- `AppHandle::execute(opts, f)` and `AppHandle::get::<T>()` exist (Read,
  `crates/ulo/src/app/handle.rs:34, 48`). Neither is new surface.
- "503 with `Connection: close` once the drain starts" is what the built service already does on
  any host: `AppService::respond` answers `render::draining` when `Execution::open` returns
  `Closed` (Read, `crates/ulo-http/src/service.rs:92-98`, `render.rs:86-93`), and `open` is
  refused from Draining on (Read, `crates/ulo/src/app/shared.rs:140-149`). Q2's rule and §9.5 hold
  without the `Embedded` doing anything.
- `HttpCx::head()` reaches the request's `http::Extensions` through `RequestHead::parts()` (Read,
  `crates/ulo-http/src/cx.rs:75`, `transport.rs:56-58`), so a `Host<T>` extractor has what it needs
  on hosts that pass `http::Request` extensions through.
- The core polls every `Server::serve` future from `App::serve` until the shutdown's outcome,
  through the drain and the close steps (Read, `crates/ulo/src/app/mod.rs:298-342`). That is what
  R5 builds on.
- actix's per-core model survives embedding under a tokio runtime: with a tokio handle current and
  no actix `System`, each worker spawns its own thread with a `new_current_thread` runtime, and
  with neither runtime actix-server panics (Read, `actix-server-2.9.7/src/worker.rs:325-352`).

## Refinements

**R1. The embedded entry point is a new type; `fw_http::Service` cannot be handed to a host.**
(Read.) `ulo_http::Service` is the inner service a pre-dispatch tower layer wraps: it takes the
request's continuation out of the extensions, and with none present it logs an error and answers
a bare 500 (`crates/ulo-http/src/tower_bridge.rs:32-68`). The app's entry point is
`AppService::call(Request)`, whose `Request` is ulo's own struct carrying `head`, `body`, `conn`
and `upgrade` (`service.rs:78-81`, `request.rs:19-27`), not an `http::Request`. The fourth
response's "`ulo_http::Service` is a tower `Service`, so it can be mounted inside an existing axum
app" and the fifth's "these share `fw_http::Service` directly" name a type that exists and does
something else. Change: `Embedded::service()` returns a new tower service over `AppService`,
named apart from `Service` (`EmbeddedService` or `Mounted`), which builds a `Request` from an
`http::Request<B>`: `ConnInfo` from the host's connection extension where one exists,
`OnUpgrade` taken from `hyper::upgrade::OnUpgrade` in the extensions as the axum backend already
does (`crates/ulo-http-axum/src/convert.rs:20-27`), and the response body mapped back. Amend the
T2 sentence in the fourth response as well.

**R2. The adapter must be known when `prepare` runs.** (Read.) `prepare` runs inside `listen()`
(`crates/ulo/src/app/mod.rs:211-220`). In the answer's own examples `fw_http::Embedded::new()`
names no adapter, `listen()` completes, and only then is the host built and the adapter met
through `embedded.service()` or `fw_http_axum::run(..)` (RESPONSE.md lines 430-438, 545-548). An
adapter-agnostic `Embedded` therefore cannot check `EmbedLimits` in `prepare`, and `.peer_addr(true)`,
`.nested_at("/api")` and `.on_miss(Miss::Forward)` have no home that `prepare` sees. Change: make
the embedding generic over the adapter as the server is over the backend,
`fw_http::Embedded<A: Embed>` beside `fw_http::Server<B: Backend>`, with `A::limits() -> EmbedLimits`
and `A::NAME`, each adapter crate exporting its alias (`fw_http_axum::Embedded`), and the three
settings on that builder before `listen()`. This changes the design's shape: §1's crate table,
§3.7's SPI and the `(server, embedded)` constructor all move with it.

**R3. The `peer_addr` refusal needs a core accessor and the transitive walk.** (Read.) The answer
has `prepare` read "`HandlerSpec` dependencies (R7)". `HandlerSpec` carries `dependencies`
(`crates/ulo/src/transport/handler.rs:29, 76-79`), but `MountedHandler` exposes `name`,
`controller`, `module`, `info` and `handler` only (`controller.rs:79-115`); the record's
`dependencies` is `pub(crate)` (`controller.rs:179, 202`), as are `Dependencies::list` and
`Requirement::reads` (`dependency/mod.rs:47-48, 115-116`). A direct read is also not the whole
check: core §6.4 has the wiring pass walk each handler's reachable execution-scoped bindings, and
that walk (`InputWalk`, `graph/scopes.rs:395-409`) is private. A `Dep<ClientAddr>` inside an
execution-scoped service a handler reaches would pass a direct check. Today `Http::inputs` declares
`ClientAddr` unconditionally (`crates/ulo-http/src/transport.rs:21-23`) while the service seeds it
only when the connection has a peer (`service.rs:99-102`), so on a host without one the read fails
at the first call as `LookupError::NotFound { kind: Input }`. Change: a core extension, numbered
with the others, `Mounted::handlers_reading::<T>() -> Vec<&MountedHandler<T>>` (or an equivalent on
`AppHandle`) built on the input walk, which `prepare` uses for `peer_addr`; until it exists the
limit is a documented runtime failure, not a `prepare` refusal.

**R4. Step 2 of the shutdown table attributes `draining()` to the wrong party.** (Read.) The core
fires `draining` and advances the phase before it calls any `Server::drain`
(`crates/ulo/src/lifecycle/shutdown.rs:231-237`). `Embedded::drain` does not "resolve
`handle.draining()`"; the `run` helper wires `handle.draining()`, already firing, to the host's
graceful stop. Rewrite the row to say so, and decide what `Embedded::drain` itself awaits: the
transport drains are polled inside the drain window beside the wait for live executions, and the
window ends when both have settled or at its deadline (`shutdown.rs:240-253`), so a `drain` that
awaits the host's full stop (axum's serve future completes only when every connection task has
ended, `axum-0.8.9/src/serve/mod.rs:296-303`) extends the window to the deadline whenever a
connection lingers, while a `drain` that returns at once leaves the host's connections to `close`.
The `Backend::drain` contract ("may return only once its connections have ended",
`crates/ulo-http/src/backend.rs:42-47`) was written for a backend whose `close` can cut
connections; an embedding's cannot, so the second reading fits it.

**R5. One owner for the host future: `Embedded::serve`, installed through the handle.** (Read.)
`Embedded::close(&self)` "awaits the host server's own future", but a future nobody polls before
`close` never serves. The core already polls each `Server::serve` from `App::serve` until the
outcome, `close` included (`crates/ulo/src/app/mod.rs:298-342`). Change: the `run` helper installs
the host future in a shared slot before calling `app.serve(signal)`; `Embedded::serve` takes it
from the slot and polls it; `Embedded::close` awaits a completion notice `serve` fires when the
host future resolves. No task is spawned, which keeps the T2 argument against detached connection
tasks intact. Consequences for the helper's signature: it needs the handle to reach the slot
(`embedded.run(app, host, signal)` or `run(app, &embedded, host, signal)`), and for axum it must
take `axum::serve::Serve` and attach `with_graceful_shutdown(draining)` itself
(`serve/mod.rs:151`), refusing a `WithGracefulShutdown` the user already built, since that would be
a second signal owner. A host future that ends with `Err` before the trigger becomes the
transport's `serve` error, which starts the shutdown naming the transport (`app/mod.rs:316-323`),
matching what a dead backend does; see Q2 for the `Ok` case.

**R6. The `close` bound and the host's own clocks.** (Read.) The row says `close` is "bounded by
what remains of the cap"; with no cap set, the default, it is `hook_timeout`, ten seconds unset
(`crates/ulo/src/lifecycle/shutdown.rs:298-310`, `transport/server.rs:58-60`, `app/mod.rs:74`).
State both. The hosts then bring a third clock: actix's `shutdown_timeout` defaults to 30 s
(`actix-web-4.15.0/src/server.rs:430`, `actix-server-2.9.7/src/builder.rs:239`); rocket's
`grace` and `mercy` default to 2 s and 3 s (Probed, e05; `rocket-0.5.1/src/config/shutdown.rs:68-96`);
salvo's `stop_graceful(timeout)` takes one (`salvo_core-0.92.2/src/server.rs:90`); axum's is
unbounded (`axum-0.8.9/src/serve/mod.rs:296-303`). None reads the app's `drain_timeout`, and
`AppHandle` does not expose it. Q1 asks who aligns them; whichever way, the table should name the
consequence: an actix host still draining at 30 s outlasts a 10 s drain, and `close` records
`TimedOut` under `hook_timeout`.

**R7. `Connection: close` on actix is dropped unless the adapter sets the connection type.**
(Read.) actix-http's HTTP/1 encoder skips a `Connection` header found in the map
(`actix-http-3.18.12/src/h1/encoder.rs:152`) and writes the connection line from the head's flags
(`encoder.rs:128`, `responses/head.rs:105-113`), set through `ResponseHead::set_connection_type`
(`head.rs:59`), reachable from `HttpResponse::head_mut` (`actix-web-4.15.0/src/response/response.rs:72`).
The drain row's "503 with `Connection: close`" holds on actix only if the adapter translates the
header into the flag. hyper honours the header on HTTP/1.1 (`hyper-1.11.1/src/proto/h1/role.rs:838-841`)
and strips it on HTTP/2, where the host's GOAWAY stands (`proto/h2/mod.rs:50`). Change: the actix
adapter's response conversion maps the header to the flag, and the conformance drain scenario
asserts the connection closes after the 503.

**R8. poem's tower bridge loses the upgrade, the peer address and the original URI; poem and salvo
are better served by native implementations.** (Read.) poem's `From<Request> for hyper::Request<BoxBody>`
copies method, URI, version, headers, extensions and body (`poem-3.1.12/src/request.rs:187-199`),
and nothing else: the upgrade future is a separate field, removed from the extensions when the
request is built (`request.rs:43, 92, 162`); `remote_addr`, `local_addr`, `scheme` and
`original_uri` are fields too (`request.rs:252, 270, 417-423`). A `poem::Endpoint` written
natively (`endpoint/endpoint.rs:21`) reads `take_upgrade()`, `remote_addr()` and `original_uri()`
(`request.rs:486, 417, 252`). salvo's bridge calls `strip_to_hyper`, which carries headers,
extensions and body (`salvo_core-0.92.2/src/http/request.rs:269-289`) and so keeps hyper's
`OnUpgrade`, which salvo leaves in the extensions (`salvo_extra-0.92.2/src/websocket.rs:351`), but
`remote_addr` is a field (`request.rs:416`) and is lost; a native `salvo::Handler` reads it. salvo's
bridge is also behind the non-default `tower-compat` feature (`salvo-0.92.2/Cargo.toml:176`).
Change: in §2's table and in "Engines and adapters", axum is the one tower embed; salvo and poem
join actix and rocket as native embeds, or keep the bridge and declare `upgrades: false,
peer_addr: false` for poem and `peer_addr: false` for salvo.

**R9. axum's `peer_addr` and prefix, as the host supplies them.** (Probed, e01; Read.)
`ConnectInfo<SocketAddr>` is inserted only by `IntoMakeServiceWithConnectInfo`
(`axum-0.8.9/src/extract/connect_info.rs:82-151`); `axum::serve(listener, router)` with a plain
`Router` inserts none (e01 prints `connect_info=false`). With `.peer_addr(true)` the user must
pass `router.into_make_service_with_connect_info::<SocketAddr>()`, so the helper's example and its
signature should take the make-service, or the documentation say so beside `.peer_addr`. For the
prefix, `nest_service` strips it (e01: `/api/users/1?x=1` arrives as `/users/1?x=1`, `/api` as `/`)
and sets `NestedPath` and `OriginalUri` on every request (`src/routing/path_router.rs:246-267,
371-384`; `original-uri` is a default feature, `Cargo.toml:81-91`), while `fallback_service` sets
`OriginalUri` and no `NestedPath` (e01). Change: the axum adapter reads `NestedPath` per request and
logs a mismatch against `.nested_at`, which `prepare` still needs for the route table and the span.

**R10. `Host<T>` and `Handled` exist on the tower hosts; actix and rocket need their own form.**
(Read.) actix's request store is `actix_http::Extensions`, and rocket's is `Request::local_cache`
(`rocket-0.5.1/src/request/request.rs:741`); neither is enumerable, so an adapter cannot copy a
host's typed values into the `http::Extensions` the app reads. `Handled` has a place on actix
(`HttpResponse::extensions_mut`, `actix-web-4.15.0/src/response/response.rs:211`) and none on
rocket, whose `Response` has no extensions; the request's local cache is the slot a fairing's
`on_response` reads (Probed, e05 compiles the write). Change: add `host_extensions` to `EmbedLimits`
with `Host<T>` refused in `prepare` where it is `false`, or give those two adapters an explicit
`.forward::<T>(fn)` registration that reads the host's store under the type the app names; and say
where `Handled` lives on rocket. See Q5 for `Host<T>`'s failure shape and Q6 for `Handled` on a
miss.

**R11. `Miss::Forward` on rocket requires the original `Data<'r>`, unread.** (Probed, e05; Read.)
`Outcome::Forward` carries `(Data<'r>, Status)` (`rocket-0.5.1/src/route/handler.rs:7, 257`), and
`Data::local` is `pub(crate)` (`src/data/data.rs:60`), so a body already read into bytes cannot be
rebuilt for the forward; `Data::open` yields a `DataStream<'r>` bound to the request's lifetime
(`data.rs:85`). The adapter can forward only a `Data` it never opened, so the app must report the
miss before anything pulls the body, and the unscoped stage runs first and may consume it or
rewrite the path, which rocket's re-routing of the original request would not see. Change: the
adapter hands the app a body that reads `Data` lazily and forwards only when nothing pulled it and
the path is unchanged, answering 404 otherwise with a `warn`; or `prepare` refuses `Miss::Forward`
beside a body-consuming unscoped entry. Q3 asks which. Rank is also worth a sentence: a `/<p..>`
route takes the lowest default precedence (`src/route/uri.rs:239-246`), so host routes already win
over an unranked catch-all, and the forward matters once the app's routes are mounted with
`Route::ranked` above them (e05 compiles that form).

**R12. "Panics in the app are caught inside it" is true of the pipeline, not of a body the host
polls.** (Read.) `dispatch` and `catch_panic` cover guards, interceptors, handlers and the
pre-dispatch entries (`crates/ulo/src/app/handle.rs:111-120`). `ExecBody::poll_frame` and the user
stream inside it run on the host's connection task with no catch (`crates/ulo-http/src/service.rs:444-457`).
A panicking streaming body reaches the host. Scope the sentence to the pipeline, and let the
conformance suite leave the body case to each host.

**R13. An embedded app reports no addresses.** (Read.) `Server::bound` defaults to none
(`crates/ulo/src/transport/server.rs:63-68`), so `App<Bound>::addresses()` is empty. Say so beside
the host's ownership of sockets, together with the consequence that port 0, inherited sockets and
`fw dev --listen` are the host's business.

**R14. The 503 before bind needs its own text and no `AppHandle`.** (Read.) `render::draining`
says "the server is shutting down" (`crates/ulo-http/src/render.rs:86-93`); a request before
`listen()` is a different condition. The handle exists before any `Mounted` or `AppHandle` does, so
the refusal is rendered from the `HttpConfig` the `Embedded` builder holds, with `Retry-After` and
a message naming the state ("the application is not yet listening").

**R15. Two more rows for `EmbedLimits`, read by the conformance suite.** (Read; Unverified where
marked.) Request bodies: rocket's `Data<'r>` must be read inside the handler future (`data.rs:85`),
so the rocket embed buffers the body under a cap, as the former adapter did at 32 MiB
(`git show master:crates/ulo-http-rocket/src/rocket_adapter.rs`); a `request_body: Streamed |
Buffered(cap)` limit lets the byte-identical rule exempt the 413 scenarios where the cap bites
first. Disconnects: `CancelReason::Disconnected` fires when the host drops the response body
(`crates/ulo-http/src/service.rs:468-475`); the hyper-based hosts drop it at the disconnect, and
the former actix, poem and rocket adapters dropped it at the next failed write (the ulo audit's
F102, recorded in the project's CLAUDE.md; Unverified for the new embeds). A
`disconnect: AtClose | AtNextWrite` limit tells the cancellation scenario what to expect. On actix
the body crosses a worker-local pump, so the drop is the pump's
(`git show master:crates/ulo-http-actix/src/actix_adapter.rs`, `pump_worker_local_body`).

**R16. actix's upgrade is a payload/body pair, not a per-request future.** (Read.) Inside an
`App`, no upgrade future exists; `HttpServiceBuilder::upgrade` sits at the actix-http level
(`actix-http-3.18.12/src/builder.rs:242`). The HTTP/1 dispatcher treats a response whose head is
`ConnectionType::Upgrade` by continuing to pipe the request payload in and the response body out
(`src/h1/dispatcher.rs:466-478, 516-528`), which is how actix's own WebSocket crates work. An
`Upgraded` for actix is therefore built from the payload stream and a channel-backed response body,
wrapped into `AsyncRead + AsyncWrite` across the `!Send` boundary (Probed, e04: `Payload` holds an
`Rc<RefCell<..>>` and a `dyn Stream` without `Send`). Name that mechanism in the limits table's
`upgrades` row, or declare `upgrades: false` for actix's first version (Q4).

**R17. rocket's upgrade is reachable, and the row can say how.** (Read.) rocket takes
`hyper::upgrade::on` before the handler runs (`rocket-0.5.1/src/server.rs:80-81`); when the
response registered a protocol (`Response::add_upgrade`, `src/response/response.rs:990`) matching
the request's `Upgrade` header (`response.rs:804`, `server.rs:91-93`), rocket writes
`Connection: Upgrade` and `Upgrade: <proto>` itself (`server.rs:193-194`), awaits the hyper upgrade
and calls `IoHandler::io(self: Pin<Box<Self>>, IoStream)` (`src/data/io_stream.rs:69-71`), which
must stay alive for the connection's life (`server.rs:197-205`); `IoStream` is `AsyncRead +
AsyncWrite` (`io_stream.rs:17`), so `Upgraded::new` accepts it (`crates/ulo-http/src/request.rs:146`).
The adapter's `IoHandler` resolves the app's `OnUpgrade` with the stream and returns when the app
drops it. Set `upgrades: true` for rocket, with the two conditions: the app's 101 must name
`websocket` for the match, and rocket overwrites the two upgrade headers.

**R18. The design body still lists five backends.** (Read.) `transports/DESIGN.md` §1 names
`fw-http-axum, -actix, -salvo, -poem, -rocket` as "One `fw_http::Backend` each" (line 27) and
§3.7's limits table has four backend rows (lines 510-515); `CAPABILITIES.md` [16] reads the same.
Change: §1 lists `fw-http-hyper` as the `Backend` and the five as `Embed` adapters; §3.7's table
keeps hyper; a §3.8 states the `Embed` SPI, `EmbedLimits` and the `Embedded<A>` server; §12 gains
the `EmbedLimits` refusals; §10's shutdown table gains the embedded column from the answer's own
table, corrected per R4 and R6.

**R19. The app-level settings are five, not two.** (Read.) `Server<B>` carries `body_limit`,
`max_inflight`, `shed_retry_after`, `challenge` and `timeout_grace` as app-level, with `tls`,
`endpoint`, `h2c` and `max_concurrent_streams` as the host's (`crates/ulo-http/src/server.rs:60-112`).
The answer lists `body_limit` and `max_inflight`. Name all five on `Embedded`, so the E0599 claim
is exact in both directions.

**R20. One `prepare` for both servers.** (Read.) `Server<B>::prepare` builds the stage, the router,
the upgrade paths and the admission (`crates/ulo-http/src/server.rs:248-306`). `Embedded::prepare`
builds the same and then checks `EmbedLimits`. Factor the shared part into one crate-private
function both call, so the conformance suite compares one route table with itself rather than two
that can drift.

## Questions

**Q1. Who aligns the host's drain clock with the app's?** `drain_timeout` is not on `AppHandle`.
Does `run` take the window and pass it to `stop_graceful(timeout)`, actix's `shutdown_timeout` and
rocket's `grace`, or are the clocks declared independent with R6's consequence documented?

**Q2. A host that stops before the trigger: error or done?** The core treats a `serve` that returns
`Ok` early as "simply done" (`crates/ulo/src/app/mod.rs:294-297`). For an embedding, that leaves an
app with nothing serving it. Should `Embedded::serve` answer `Err` for a host that ends before
`is_draining()`, so the app closes naming the transport?

**Q3. `Miss::Forward` beside the unscoped stage (R11):** forward only when the body is untouched
and the path unchanged, else 404 with a `warn`; or refuse `Miss::Forward` in `prepare` when any
unscoped entry consumes the body or may rewrite? The second is static; the first keeps `RequestId`
style entries working.

**Q4. Does the actix adapter declare `upgrades: false` in its first version**, or ship the
payload/body bridge of R16?

**Q5. What is `Host<T>`'s failure shape?** As a `FromCall<Http>` extractor an absent value is
`ExtractError::Missing`, a 400 by §3.6, while a missing host extension is a deployment fault. Does
absence render 500, is `Option<Host<T>>` the only spelling, or is `Host<T>` checked in `prepare`
against what the adapter declares the host provides?

**Q6. `Handled` on a miss:** no route and no handler matched. Optional fields, or a separate
`Missed` marker? Does the conformance suite compare response extensions, which the byte-identical
rule does not cover?

**Q7. `EmbedLimits` and `BackendLimits` share one field (`upgrades`).** Separate structs as the
answer implies, or one struct with host-owned fields marked not applicable? Separate keeps the
E0599 story; one struct keeps the `prepare` code shared (R20).

**Q8. The nest prefix itself.** On axum a request to exactly `/api` arrives as `/` (Probed, e01).
Is `/` a route the embedded app may own, and does `HttpCx::mount_prefix()` join as `/api` plus the
route, with the route table's trailing-slash rule deciding `/api/` against `/api`?

**Q9. Does the conformance suite run the tower hosts under HTTP/2 as well?** `Connection: close`
is stripped there and GOAWAY carries the drain (R7), so the drain scenario has two shapes per host.

## Appendix: probes

The probe crate is `docs/experiments/di-redesign/probes/embedding/`, a crate of its own beside the
core probes with the host frameworks as dependencies, locked and built offline from the registry
cache. Not committed.

| Probe | Result |
| --- | --- |
| `e01_axum_nest_strip` (run) | `/api/users/1?x=1` reaches the nested service as `uri=/users/1?x=1`, `OriginalUri=/api/users/1?x=1`, `NestedPath=/api`, `ConnectInfo` absent; `/api` arrives as `uri=/`; `/other/path` reaches the fallback with the full URI, `OriginalUri` set, no `NestedPath`. |
| `e03_salvo_compat_bounds` (compiled, run) | A `tower::Service<hyper::Request<ReqBody>>` with `Error = Infallible`, a boxed `Send` future and `Clone + Send + Sync` passes `TowerServiceCompat::compat` and mounts on `Router::with_path("api/{**rest}")`. |
| `e04_actix_payload_send_fails` (expected compile error) | `actix_web::dev::Payload` is `!Send`: `Rc<RefCell<h1::payload::Inner>>` and `dyn Stream<Item = Result<Bytes, PayloadError>>` without `Send`. |
| `e05_rocket_forward_and_cache` (compiled, run) | A `Handler` returning `Outcome::Forward((data, Status::NotFound))` with the original `Data`, writing a typed value through `req.local_cache`, mounted with `Route::ranked(10, ..)`; `Shutdown::default()` prints `ctrlc=true grace=2 mercy=3`. |
| poem bridge | Not built: poem's `tower-compat` feature depends on tower 0.4.13, which is not in the cache. Every poem claim above is Read from `poem-3.1.12/src`. |

Not verified here: the `Send`-ness of actix's `Server` future (its `fut` is a `BoxFuture<'static, ..>`,
`actix-server-2.9.7/src/server.rs:137-140`, which is `Send`) and of rocket's `launch()` future,
both needed for R5's slot; and the host-by-host moment at which a dropped response body is
observed (R15).
