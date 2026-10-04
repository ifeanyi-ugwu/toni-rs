# Divergences: race 2a, the post-compile follow-ups T3, T10, T12 and T24

`transports/DIVERGENCES.md` left four items for after the first compile, and the fourth response in
`transports/RESPONSE.md` answered them: T3 and T24 use the core's `Bound` vocabulary rather than
`Option`, with a matching `Default | Max(n) | Unlimited` shape for the stream limit; T10 and T12 as
recommended. `transports/DESIGN.md` §3.6 "Server settings" states the T3/T24 result; §3.1 states
T10's limit and what replaces it; §X3 states T12's report text. This build implements them.

Files changed: `crates/ulo-http/src/{backend.rs, server.rs, cx.rs, service.rs, sse.rs, embed.rs,
lib.rs}`, `crates/ulo-http/Cargo.toml`, `crates/ulo-http-hyper/src/{backend.rs, listener.rs,
lib.rs}`, `crates/ulo-transport/src/__private.rs`, `crates/ulo-handler-codegen/src/emit.rs`.

## The signatures

```rust
// ulo_http (new)
pub enum Count { #[default] Default, Max(u32), Unlimited }      // Clone, Copy, Debug, Default, PartialEq, Eq, Hash
impl HttpConfig {
    pub fn header_timeout_after(&self) -> Option<Duration>;      // 30 s at Default, None at Unbounded
    pub fn handshake_timeout_after(&self) -> Option<Duration>;   // the same
}
impl HttpCx { pub fn timer(&self) -> &Arc<dyn Timer>; }

// ulo_http::HttpConfig (#[non_exhaustive]), changed and new fields
pub max_concurrent_streams: Count,                               // was Option<u32>
pub header_timeout: Bound,                                       // new
pub handshake_timeout: Bound,                                    // new

// ulo_http::Server<B>
pub fn max_concurrent_streams(self, streams: Count) -> Self;     // was (self, streams: u32)
pub fn header_timeout(self, timeout: Bound) -> Self;             // new
pub fn handshake_timeout(self, timeout: Bound) -> Self;          // new

// ulo_transport::__private::Param<T, M> (macro protocol)
fn dependencies_named(d: &mut Dependencies, name: &'static str); // replaces fn dependencies(d)
```

## What each item does

1. **T3, the two timeouts.** `header_timeout` and `handshake_timeout` are `Bound`s on the server,
   both `Bound::Default` unset. The hyper backend sets hyper's HTTP/1.1 header-read timeout on both
   connection builders in every case: 30 s at `Default`, `d` at `After(d)`, cleared at `Unbounded`.
   The 30 s is therefore the server's value, not hyper's. The TLS handshake runs under
   `tokio::time::timeout` with the resolved duration, or with no timeout at `Unbounded`. Either way
   it is still abandoned at the drain. A timed-out handshake is logged at `debug` with the peer and
   the duration, as before.
2. **T24, the stream limit.** `max_concurrent_streams` is a `Count`. `Default` calls nothing on
   hyper's HTTP/2 builder, leaving hyper's own value (200 in hyper 1.11.1). `Max(n)` sets `n`.
   `Unlimited` passes `None`, which sends no `SETTINGS_MAX_CONCURRENT_STREAMS`.
3. **T10, SSE keep-alive.** `HttpCx` carries the app's `Timer`, the same `Arc` the route timeout
   already reads from `ServiceInner`, and `HttpCx::timer()` exposes it. The keep-alive requests
   `timer.sleep(period)` and no longer reads tokio's clock. `ulo-http` drops tokio's `time`
   feature, so nothing in the crate picks a runtime the app did not.
4. **T12, named parameters.** The generated `dependencies` block calls
   `Param::dependencies_named(&mut d, "<name>")`, where the name is the parameter as the signature
   writes it. A container-read parameter (`ViaContainer`) declares `d.param::<S>(name)`. A
   `FromCall` parameter (`ViaCall`) keeps `P::dependencies(d)`. The two reports now read
   ``NamedParams::named (param `svc`)`` and
   ``CallerParams::caller (Http) → Dep<CallerId> (param `caller`)``.

## Decisions for sign-off

### 1. The count type is `ulo_http::Count`, over `u32`

- **Written:** `Count { Default, Max(u32), Unlimited }` in `ulo-http`, exported at the crate root
  beside `HttpConfig`. It is not `#[non_exhaustive]`, matching `Bound`.
- **Why:** the answer names the shape but not the type. DESIGN §3.6 says "a count is
  `Default | Max(n) | Unlimited`", so the type is named for what it bounds in general. It lives in
  `ulo-http` because its only reader is an HTTP setting; `Bound` is in the core because the core
  times things with it.
- **Alternatives:** put it in the core beside `Bound`, which gRPC's stream limit could later share;
  make it generic over the integer, which `max_inflight` (`usize`) would need if it moves to the
  same shape (divergence 2).

### 2. `max_concurrent_streams` takes a `Count`, with no `From<u32>`

- **Written:** `.max_concurrent_streams(Count::Max(100))`. A call written as
  `.max_concurrent_streams(100)` no longer compiles.
- **Why:** the builder follows `timeout_grace(Bound)`, which takes the enum and has no
  `From<Duration>`. A `From<u32>` would let `100` compile, but it would add a second spelling of
  `Max(100)` alongside the enum.

### 3. Backends read the timeouts through `HttpConfig::{header,handshake}_timeout_after`

- **Written:** two public methods that map `Bound` to `Option<Duration>` and own the 30 s default.
  The raw `Bound` fields stay public for a backend that wants them.
- **Why:** the 30 s is ulo's default, stated in DESIGN §3.6. Every backend should get the same
  value without restating the constant, and `None` means exactly "no clock" at this point, after
  `Default` has been resolved. `timeout_grace` has the `pub(crate)` equivalent `grace()`, because
  only `ulo-http` reads it.

### 4. The keep-alive period starts at the first pending poll after a write

- **Written:** a write drops the running sleep. The next poll that finds the stream pending
  requests a new `timer.sleep(period)`. When that sleep resolves, `: keepalive` is written and the
  sleep is dropped.
- **Why:** `Timer` offers `sleep(d)` and no resettable sleep. Resetting at the write would need
  either a new boxed sleep per event or a deadline read from `timer.now()`, re-armed when the old
  sleep fires early. With the pending-poll start, events written back to back request no sleep at
  all, and the clock starts later than the write only by the gap until the next poll. The gap
  grows when a slow reader holds the body back, and a keep-alive is pointless then because bytes
  are already queued.
- **Changed behaviour:** the tokio version reset its `Sleep` to `now + period` at every write. The
  difference is the poll gap above.

### 5. `Param::dependencies` is replaced, not kept beside `dependencies_named`

- **Written:** the trait has one dependencies method, `dependencies_named`.
- **Why:** the generated code was the trait's only caller. A second, unnamed method would be
  protocol surface that nothing calls. A transport crate that writes `Param` impls by hand gains a
  parameter; none in the workspace does.

### 6. What a parameter's name is, and what stays positional

- **Written:** the name is `params::Param::name`: the identifier, unraw'd (`r#type` gives `type`),
  `mut` dropped, and for a destructuring pattern the pattern as token text, so a user's
  `FromContainer` tuple struct written `Config(c): Config` prints ``param `Config (c)` ``. A
  `ViaCall` parameter that reads the container, such as `Option<Injected<S>>`, stays positional
  through `Dependencies::add`. Its `#n` counts every entry before it, named ones included.
- **Why:** the brief's fix names `ViaCall` as `P::dependencies(d)`, and a `FromCall` impl declares
  its reads with no name to pass. The core's container types have private fields, so the
  token-text case is reached only by a user type.

## Divergences

1. **DESIGN §3.1 still describes the tokio clock as the standing limit.** It says the keep-alive
   clock "is `tokio::time::Sleep` ... a known limit that stands until the app's `Timer` is reachable
   from `HttpCx` and replaces it". The build makes that replacement, which T10's recommended
   follow-up and the brief name. The sentence needs replacing with "timed by the app's `Timer`,
   which `HttpCx::timer()` reads". DESIGN.md is not edited here.
2. **DESIGN §3.6 says "No setting is an `Option`"; `max_inflight` still is one.**
   `HttpConfig::max_inflight` stays `Option<usize>`, and the builder still takes a bare `usize`.
   The signed answer covers T3 and T24. `None` there has one meaning, no bound, since no default
   bound exists. The rule as written still covers it. Moving it to the count shape needs a `usize`
   count (decision 1's generic alternative) or a second type.
3. **`header_timeout` bounds an HTTP/1.1 head only.** DESIGN §3.6 says it bounds "a request head".
   hyper 1.11.1's HTTP/2 builder has no head-read timeout, so an HTTP/2 connection (ALPN `h2`, or
   h2c) runs no such clock. The `ulo-http-hyper` crate doc says so.
4. **`Bound::After(Duration::ZERO)` is passed through unchecked** for both timeouts. Neither
   DESIGN nor the answer refuses it. Nothing here tests what hyper or tokio does with zero.

## Not covered

- HTTP/2 `RST_STREAM(REFUSED_STREAM)` for a stream over the limit is hyper's. The checks read only
  the SETTINGS value the server advertises.
- An `Embedded` app has neither timeout nor `max_concurrent_streams`, because the host owns the
  connection. The `embed` module doc now lists all three among the host's settings.

## Verification

- `cargo check --workspace --all-targets` and `cargo +1.88 check --workspace --all-targets` pass.
  The warning lists (cargo JSON, deduplicated by crate, file and message) are identical to HEAD's:
  17 entries on each toolchain, all in `crates/ulo/src`. The comparison was checked against a
  planted unused import in `crates/ulo-http/src/cx.rs`, which it reported as two new entries, and
  the lists matched again once the import was removed.
- `cargo test --workspace` passes.
- `RUSTDOCFLAGS="-D warnings" cargo doc -p ulo -p ulo-http -p ulo-http-hyper --no-deps` passes, as
  does the same for `ulo-handler-codegen` and `ulo-transport`.
- The scratch crate `embed` (`#![deny(warnings)]`, its own `[workspace]`, now also depending on
  `ulo-net` and `rcgen`) keeps its 60 checks and adds 20. All 80 pass on rustc stable and on 1.88,
  with identical labels. The new checks:

| Check | Server or app | Result |
| --- | --- | --- |
| A partial head closes the connection | `header_timeout(After(300ms))` | closed at 302 ms |
| A head arriving in time is answered | same | `200` |
| A TCP connection that sends no ClientHello closes | `tls`, `handshake_timeout(After(300ms))` | closed at 302 ms |
| A partial head closes the connection | unset | closed at 30.003 s |
| A partial head outlives 30 s, and the head completed after 34 s is answered | `header_timeout(Unbounded)` | open, `200` |
| No ClientHello closes the connection | `tls`, unset | closed at 30.003 s |
| No ClientHello outlives 30 s | `tls`, `handshake_timeout(Unbounded)` | open at 34 s |
| The server's first SETTINGS frame over h2c | `Count::Max(7)` / unset / `Count::Default` / `Count::Unlimited` | `7` / `200` / `200` / absent |
| SSE: nothing is written before the fake `Timer` fires; one sleep of the period requested | `Embedded`, `App::timer(FakeTimer)`, `keep_alive(3600 s)` | as stated |
| SSE: firing the sleep writes `: keepalive`; the next period is a fresh sleep | same | as stated |
| SSE: an event is written and drops the running sleep; the period restarts; firing writes `: keepalive` | same | as stated |
| Wiring report for an unbound `svc: Dep<Unbound>` after `id: Path<u32>` | `#[routes]` controller | ``NamedParams::named (param `svc`)``, no `param #` |
| Wiring report for `caller: Dep<CallerId>`, which `Rpc` seeds, on an HTTP handler | `#[routes]` controller and `RpcApi` | ``CallerParams::caller (Http) → Dep<CallerId> (param `caller`)`` |

The fake `Timer` records each `sleep(d)` with a oneshot sender and resolves a sleep only when the
check fires it. A keep-alive of an hour cannot pass by tokio's clock during the run. The
30-second probes start first and are read last, so one run takes about 35 s.

Against known violations, each one planted alone into `crates/` and the scratch crate rerun on
stable and on 1.88, with identical results on both:

| Plant | Fails |
| --- | --- |
| T3: hyper builders keep hyper's own header-read timeout; handshake fixed at 30 s | the two `After(300ms)` checks (still open at 5 s) and the two `Unbounded` checks (closed at 30.00 s); the 30 s defaults still pass |
| T24: `Default` passed to hyper as `None`, `Unlimited` left at hyper's default | "unset sends hyper's own value" (absent) and "`Unlimited` sends no limit" (`200`) |
| T10: `sse.rs` and `Cargo.toml` at HEAD, keep-alive on `tokio::time::Sleep` | all six SSE checks: no sleep requested from the app's `Timer`, no `: keepalive` when it fires |
| T12: `ViaContainer` declares through `Dependencies::add`, as at HEAD | both report checks, which print `param #1` |

No check outside its item failed under any plant. Each file was restored by `cp` from a saved
copy and touched, and `cmp` confirmed it byte-identical before the final runs.

## Second round: the sixteenth response

`transports/RESPONSE.md` "Sixteenth response" accepted decisions 2 to 5, moved `Count` to
`ulo-transport` (decision 1), changed the fallback number of decision 6, and asked for three of
this build's findings to be acted on: `max_inflight` as a `Count`, the HTTP/1.1 scope of
`header_timeout` on the setting itself, and a refusal of a zero timeout in `prepare`. The core's
zero-timeout check is F313 in the workspace gaps ledger and is not built here.

Files changed: `crates/ulo-transport/src/{count.rs (new), lib.rs, __private.rs}`,
`crates/ulo-http/src/{backend.rs, server.rs, embed.rs, sse.rs, lib.rs}`,
`crates/ulo-http-hyper/{Cargo.toml, src/backend.rs}`, `crates/ulo/src/{__private.rs, graph/mod.rs}`,
`crates/ulo-handler-codegen/src/{params.rs, emit.rs}`, `Cargo.lock`.

### The signatures

```rust
// ulo_transport (moved from ulo_http, unchanged otherwise)
pub enum Count { #[default] Default, Max(u32), Unlimited }      // Clone, Copy, Debug, Default, PartialEq, Eq, Hash

// ulo_http::HttpConfig
pub max_inflight: Count,                                         // was Option<usize>

// ulo_http::Server<B> and ulo_http::embed::Embedded<A>
pub fn max_inflight(self, requests: Count) -> Self;              // was (self, requests: usize)

// ulo::__private (macro protocol)
pub enum HandlerParam { Named(&'static str), At(usize) }
pub fn handler_param(d: &mut Dependencies, param: HandlerParam, declare: impl FnOnce(&mut Dependencies));

// ulo_transport::__private::Param<T, M> (macro protocol)
fn dependencies(d: &mut Dependencies, param: HandlerParam);      // replaces fn dependencies_named(d, name)
```

### What each item does

1. **`Count` in `ulo-transport`.** `ulo_transport::Count`, exported at the crate root beside
   `Admission`. Its `Default` variant now reads "the setting's own default", which each setting
   documents. `ulo-http` no longer exports it, and `ulo-http-hyper` gains a `ulo-transport`
   dependency for its `match` on `max_concurrent_streams`.
2. **`max_inflight` as a `Count`.** `Count::Default` is the value unset and bounds nothing, which
   is what `None` meant: the server and the embedding admitted every request. `Count::Unlimited`
   behaves the same today. `Count::Max(n)` sheds over `n`. The conversion to `Admission::new`'s
   `Option<usize>` happens in one place, the crate-private `HttpConfig::inflight_limit`, with
   `usize::try_from(n)`, which cannot fail on a 32- or 64-bit target.
3. **The `keep_alive` doc.** `Sse::keep_alive` now states the guarantee: a comment after at least
   `every` of idleness, `every` after the last write or a little later, never sooner, since the
   period starts when the stream is next found waiting. It advises a period comfortably below the
   shortest proxy idle timeout in the deployment. The `IntoReply` impl's doc says the same in its
   own sentence.
4. **`header_timeout`'s scope.** "HTTP/1.1 only; HTTP/2 connections are bounded by the backend's
   own HTTP/2 limits." is on `HttpConfig::header_timeout` and `Server::header_timeout`.
5. **Zero timeouts refused.** `prepare_app`, which `Server<B>::prepare` and `Embedded<A>::prepare`
   both call, pushes one `Configure` failure per timeout equal to `Bound::After(Duration::ZERO)`:
   ``.header_timeout(Bound::After(Duration::ZERO))` would close every connection before its first
   request head; write `Bound::Unbounded` to turn the timeout off``, and the same form for
   `handshake_timeout` ("drop every TLS connection before its handshake") and `timeout_grace`
   ("send the 504 before any error handler could answer a route timeout"). They join the other
   `prepare` failures in the one report. Each of the three builder methods documents the refusal.
6. **Parameter names everywhere.** The generated `dependencies` block passes
   `HandlerParam::Named("<ident>")` for a parameter bound to an identifier and
   `HandlerParam::At(<index>)` for a destructuring pattern, `index` counted from 0 among the
   parameters after the receiver. Both `Param` impls go through `ulo::__private::handler_param`,
   which runs the declaration and relabels every record it added: `ViaCall` declares through
   `P::dependencies`, `ViaContainer` through `Dependencies::add`. A call-read parameter such as
   `svc: Injected<Dep<Unbound>>` now reports ``InjectedParams::injected (param `svc`)``, and
   `Injected(svc): Injected<Dep<Unbound>>` written third reports
   `DestructuredParams::destructured (param #3)`.

### Decisions for sign-off

#### 1. `ulo-http` does not re-export `Count`

- **Written:** users write `ulo_transport::Count`.
- **Why:** `ulo-http` re-exports no type of a sibling `ulo` crate. Its builder already takes
  `ulo::Bound` and `ulo_net::Tls` under their own crates' paths, and the transport-neutral types it
  reads (`CallError`, `ErrorKind`, `Admission`, `Injected`) are reached through `ulo_transport`.
  Its only re-exports are of external crates, `bytes::Bytes` and four `http` types. A re-export
  would give `Count` two paths where `Bound` has one.
- **Alternative:** `pub use ulo_transport::Count` at the `ulo-http` root, saving an application
  the `ulo-transport` dependency when it uses no other neutral type.

#### 2. The zero refusal covers the three `Bound` settings and no `Duration` setting

- **Written:** `header_timeout`, `handshake_timeout` and `timeout_grace`, which are every `Bound`
  `HttpConfig` holds. The embedding refuses `timeout_grace`; it has no builder for the other two,
  so their `Bound::Default` never trips the check there.
- **Not refused:** the `Duration` settings. `shed_retry_after(Duration::ZERO)` is a valid
  `Retry-After: 0`. `Sse::keep_alive(Duration::ZERO)` is built per response, after `prepare`, so
  `prepare` cannot see it. `#[meta(Timeout(Duration::ZERO))]` is visible in `prepare`, since the
  router reads each route's metadata there, and would cancel every request on the route; it falls
  under the same refuse-what-`prepare`-can-see rule but is not a `Bound`, so it is left for an
  answer.

#### 3. `Count::Max(0)` is accepted

- **Written:** `max_inflight(Count::Max(0))` sheds every request, and
  `max_concurrent_streams(Count::Max(0))` advertises zero streams. Neither is refused.
- **Why:** the answer asked for zero refusals on timeouts only. Both are visible in `prepare`, and
  both make the server refuse everything: `max_inflight` sheds every request, and RFC 9113 §6.5.2
  allows a zero `SETTINGS_MAX_CONCURRENT_STREAMS` but says a server SHOULD set it only for short
  durations and close the connection if it does not wish to accept requests. A configured zero is
  not short.
- **Alternative:** refuse `Count::Max(0)` on both in `prepare` with the hint "write
  `Count::Unlimited` to remove the limit".

#### 4. The compile-time body diagnostic keeps the pattern text

- **Written:** codegen's `Param` carries `name`, the identifier or the pattern as token text, and
  a new `ident`, the identifier only. The wiring report reads `ident` and falls back to the
  position. The `CONSUMES_BODY` assertion still reads `name`, so two body-consuming parameters
  written `Json(a)` and `Form(b)` are named as written in that compile error.
- **Why:** the compile error points at the signature, where the pattern is in front of the
  reader. A wiring report has no span to point at, which is the case the answer's number serves.
- **Changed behaviour:** the previous round's ``param `Config (c)` `` for a destructured
  `FromContainer` tuple struct is now `param #n`.

#### 5. `handler_param` relabels whatever the declaration added

- **Written:** every record `declare` pushes takes the parameter's label, whatever label the
  `FromCall` impl gave it. One parameter whose impl declares two reads prints the same name for
  both.
- **Why:** the reads belong to that parameter, and a `FromCall` impl has no name of its own to
  give them. Relabelling after the fact keeps `FromCall::dependencies(d)` unchanged for transport
  crates.

### Divergences

1. **DESIGN §3.6 does not say where `Count` lives, what `max_inflight` at `Default` means, that a
   zero timeout is refused, or that `header_timeout` bounds HTTP/1.1 only.** Each is now in the
   rustdoc of the setting. DESIGN.md is not edited here.
2. **DESIGN §X3 names a handler parameter "by its identifier" and has no rule for a destructuring
   pattern.** The build prints `param #n` from the signature position, as the answer specifies.
3. **DESIGN §3.1 still describes the keep-alive clock as `tokio::time::Sleep`.** First-round
   divergence 1 stands.

### Not covered

- The core's zero-timeout check (F313).
- The old crates' `with_max_inflight(usize)` on `ulo-rpc-tcp`, `ulo-rpc-udp` and `ulo-grpc`, which
  this branch has not rebuilt.

### Verification

- `cargo check --workspace --all-targets` and `cargo +1.88 check --workspace --all-targets` pass.
  The warning lists (cargo JSON, deduplicated by crate, file and message) are identical to HEAD's,
  17 entries on each toolchain, all in `crates/ulo/src`. The comparison reported a planted unused
  import in `crates/ulo-http/src/cx.rs` as one new entry, and matched HEAD again once it was
  removed.
- `cargo test --workspace` passes: 20 test binaries, 3 passed, 12 ignored, none failed.
- `RUSTDOCFLAGS="-D warnings" cargo doc -p ulo -p ulo-transport -p ulo-http -p ulo-http-hyper
  --no-deps` passes, as does the same for `ulo-handler-codegen`.
- The scratch crate `embed` (`#![deny(warnings)]`, now also depending on `ulo-transport`) keeps
  its 80 checks, with `Count` imported from `ulo_transport` and the round-2 shedding check written
  `max_inflight(Count::Max(1))`, and adds 10. All 90 pass on rustc stable and on 1.88, with
  identical labels. The new checks:

| Check | Server or app | Result |
| --- | --- | --- |
| The second request while the first response is held | `Embedded`, `max_inflight(Count::Max(1))` | `503` |
| The same | `max_inflight(Count::Unlimited)` | `200` |
| The same | `max_inflight(Count::Default)` | `200` |
| `listen()` refuses, naming ``.header_timeout(Bound::After(Duration::ZERO))`` and the hint | hyper, `header_timeout(After(0))` | refused |
| The same for `handshake_timeout` | hyper, `handshake_timeout(After(0))` | refused |
| The same for `timeout_grace` | hyper, `timeout_grace(After(0))` | refused |
| The same for `timeout_grace` | `Embedded`, `timeout_grace(After(0))` | refused |
| All three zeros give `3 problems:` and the hint three times | hyper, all three at `After(0)` | as stated |
| Wiring report for an unbound `svc: Injected<Dep<Unbound>>` after `id: Path<u32>` | `#[routes]` controller | ``InjectedParams::injected (param `svc`)``, no `param #` |
| Wiring report for `Injected(svc): Injected<Dep<Unbound>>` after `Path(id): Path<u32>` and `cx: HttpCx` | `#[routes]` controller | `DestructuredParams::destructured (param #3)`, no parameter printed by name |

Against known violations, each one planted alone into `crates/` and the scratch crate rerun on
stable and on 1.88, with identical results on both:

| Plant | Fails |
| --- | --- |
| `inflight_limit` maps `Count::Max(_)` to `None` | the round-2 shedding check and `Count::Max(1)` (both `200`) |
| `inflight_limit` maps `Count::Unlimited` to `Some(1)` | `Count::Unlimited` (`503`) |
| `prepare_app` does not call `check_zero_timeouts` | all five zero-timeout checks (`listen()` succeeds) |
| `check_zero_timeouts` checks `header_timeout` only | the `handshake_timeout`, both `timeout_grace` and the three-in-one checks |
| `ViaCall` declares through `P::dependencies(d)`, as at HEAD | both report checks, which print `param #1` |
| `handler_param` numbers `HandlerParam::At` by the records before it | the destructuring check, which prints `param #1`; the named one still prints ``param `svc` `` |

No check outside its item failed under any plant. Each file was restored by `cp` from a saved
copy and touched, and `cmp` confirmed it byte-identical before the final runs.

## Third round: the seventeenth response

`transports/RESPONSE.md` "Seventeenth response" accepted second-round decisions 4 and 5 and
reversed 1 to 3: `ulo-http` re-exports the types its builders take under a `config` module, a
route's `Timeout(Duration::ZERO)` is refused in `prepare`, a zero `Sse::keep_alive` means off, and
`Count::Max(0)` is refused on `max_inflight` and `max_concurrent_streams`.

Files changed: `crates/ulo-http/src/{config.rs (new), lib.rs, limits.rs, router/mod.rs, server.rs,
sse.rs}`.

### The signatures

```rust
// ulo_http::config (new), each item #[doc(no_inline)]
pub use ulo::Bound;
pub use ulo_net::{Endpoint, EndpointSpec, Tls};
pub use ulo_transport::Count;
```

No other signature changes.

### What each item does

1. **`ulo_http::config`.** The module re-exports the five types above. Its doc says each name is
   the defining crate's type, that either path fits wherever the other is expected, and that the
   defining crates stay the canonical homes. The crate doc lists the module beside `embed`.
2. **A zero route timeout refused.** `Router::build`, which `prepare_app` runs for the server and
   the embedding alike, pushes one `Configure` failure per route whose timeout is
   `Duration::ZERO`: `` `Users::slow` on `GET /slow`: `#[meta(Timeout(Duration::ZERO))]` on the
   handler would cancel every request before the handler could answer; remove the
   `#[meta(Timeout(..))]` to turn the timeout off``. When the zero comes from the `#[routes]` impl,
   "on the handler" reads "on the `#[routes]` impl". The `Timeout` doc states the refusal.
3. **A zero keep-alive is off.** `Sse::keep_alive(Duration::ZERO)` stores no keep-alive, so the
   body requests no sleep and writes no comment. A later zero call also turns off an earlier
   non-zero one. The method's doc says so and why: a zero period would write a comment every time
   the stream is found waiting.
4. **`Count::Max(0)` refused.** `prepare_app` pushes `` `.max_inflight(Count::Max(0))` would shed
   every request; write `Count::Unlimited` to remove the limit`` and the same form for
   `max_concurrent_streams` ("let no HTTP/2 stream open"). The embedding refuses `max_inflight`.
   It has no `max_concurrent_streams` builder, so its `Count::Default` never trips the check. The
   two server builders document the refusal; `Embedded::max_inflight` points at
   `Server::max_inflight`, as its `timeout_grace` does.

### Decisions for sign-off

#### 1. `config` also carries `Endpoint` and `EndpointSpec`

- **Written:** `Bound`, `Count`, `Tls`, `Endpoint` and `EndpointSpec`.
- **Why:** `Server::new`, `Server::with_backend` and `Server::endpoint` take
  `impl Into<EndpointSpec>`. Text parses as a socket address only, so an inherited socket needs
  `Endpoint::inherited(..)` or `Endpoint::inherited_index(..)`, which need `ulo-net` without the
  re-export. That is the same cost the response gives for `Tls`.
- **Left out:** `ListenerName`, which an app writes only to match on `Endpoint::Inherited`, since
  the two constructors build it. `TlsError` and `EndpointError`, which no builder returns:
  `prepare` writes them into its report. `AppHandle` and `BoxError` on `embed::Handle`, which are
  not configuration, and every app depends on `ulo` for `App::builder`. The backend SPI's
  `BoundListener` and `TlsAcceptor`, which a backend crate names, not an app.
- **Alternative:** the three types the response names, leaving inherited sockets on `ulo-net`.

#### 2. The re-exports are `#[doc(no_inline)]`

- **Written:** `ulo_http::config` renders as a list of re-exports, each linking to the type's page
  in its defining crate.
- **Why:** the response names the defining crates as the canonical homes. Without the attribute,
  rustdoc inlines a cross-crate re-export: a doc build of the module without it generated
  `enum.Bound.html`, `enum.Count.html`, `enum.Endpoint.html`, `struct.EndpointSpec.html` and
  `struct.Tls.html` under `ulo_http/config/`, a second copy of each type's docs. With it the
  directory holds only `index.html`.
- **Alternative:** inline, so a reader of `ulo-http`'s docs sees each type without leaving the
  crate.

#### 3. The route refusal's hint is to remove the declaration

- **Written:** "remove the `#[meta(Timeout(..))]` to turn the timeout off".
- **Why:** a route timeout has no off value. `Timeout` wraps a `Duration`, and the router reads
  `info.meta::<Timeout>()` as `Option<Duration>`, `None` being no timeout. The only spelling of
  off is no declaration. `Metadata::get` answers the method's declaration over the controller's,
  so a handler can replace a timeout its `#[routes]` impl declares but cannot lift it. The
  `Timeout` doc states this.
- **Open:** whether a handler needs a way to lift an inherited timeout, such as a long-running
  stream under a controller-wide `Timeout`. Nothing in this round adds one.

#### 4. The refusal reads the timeout the route runs with, once per route

- **Written:** the check reads `info.meta::<Timeout>()`, the most specific declaration. A zero on
  the `#[routes]` impl is refused for each route that inherits it, and not for a handler that
  declares its own non-zero `Timeout`. A zero on an impl whose every handler overrides it is
  accepted, since no route runs with it. An impl with five inheriting routes gives five failures.
- **Why:** the response's reason for the refusal is that the zero "would cancel every request on
  that route". The route is the unit that fails, and naming each route matches the other
  route-table failures.
- **Alternatives:** refuse any declared zero, whether or not a route runs with it; give one
  failure per impl, naming its routes.

#### 5. The failure names the tier that declared the zero

- **Written:** "on the handler" or "on the `#[routes]` impl", read from `Metadata::entries`, which
  carries each declaration's type name and tier.
- **Why:** the fix is to remove the declaration, and a reader looking at the handler would not
  find a zero inherited from the impl.

### Divergences

1. **DESIGN §3.6 does not mention `ulo_http::config`, the zero route-timeout refusal, the
   `Count::Max(0)` refusal or the zero keep-alive.** Each is in the rustdoc of the item. DESIGN.md
   is not edited here.
2. **Second-round divergences 1 to 3 stand.**

### Not covered

- A zero keep-alive under a real clock. The fake `Timer` resolves a sleep only when a check fires
  it, so the planted violation below shows a sleep of zero requested, not the loop a real clock
  would run.
- The core's zero-timeout check (F313).

### Verification

- `cargo check --workspace --all-targets` and `cargo +1.88 check --workspace --all-targets` pass.
  The warning lists (cargo JSON, deduplicated by crate, file and message) are identical to HEAD's,
  17 entries on each toolchain, all in `crates/ulo/src`. HEAD's lists came from a `git archive`
  export of `1e1f01ad`. The comparison reported a planted unused import in
  `crates/ulo-http/src/config.rs` as two new entries, and matched HEAD again once it was removed.
- `cargo test --workspace` passes: 20 test binaries, 3 passed, 18 ignored, none failed. Against
  HEAD's run, the one difference is the `ignore` example in `config.rs`.
- `RUSTDOCFLAGS="-D warnings" cargo doc -p ulo -p ulo-transport -p ulo-http -p ulo-http-hyper
  --no-deps` passes.
- The scratch crate `embed` (`#![deny(warnings)]`) keeps its 90 checks and adds 8. All 98 pass on
  rustc stable and on 1.88, with identical labels. Round 10 imports `Count` and `Tls` from
  `ulo_http::config` and round 11 its zero `Bound`. Those values reach `Embedded::max_inflight`
  and `Server::max_concurrent_streams`, which take `ulo_transport::Count`, `Server::tls`, which
  takes `ulo_net::Tls`, and the three timeout builders, which take `ulo::Bound`. The crate
  compiling shows the re-exports are those types. The new checks:

| Check | Server or app | Result |
| --- | --- | --- |
| The five `config` names have the defining crates' `TypeId`s | — | as stated |
| `listen()` refuses `` `ZeroTimeoutOnHandler::zero` on `GET /zero`: `#[meta(Timeout(Duration::ZERO))]` on the handler would cancel every request`` with the hint; the sibling route with `Timeout(5 s)` is not named | hyper, server built from `EndpointSpec::from("127.0.0.1:0")` | refused |
| `listen()` refuses the route inheriting a zero from the `#[routes]` impl, "on the `#[routes]` impl", with the hint; the route declaring `Timeout(5 s)` is not named | `Embedded` | refused |
| `keep_alive(Duration::ZERO)`: nothing written in 300 ms while the stream waits; no sleep requested from the fake `Timer` | `Embedded`, `App::timer(FakeTimer)` | `200`, none written, 0 sleeps |
| The same stream: an event is written, then nothing in 300 ms and still no sleep | same | `data: hello`, 0 sleeps |
| `listen()` refuses `` `.max_inflight(Count::Max(0))` would shed every request`` with the hint | hyper, server built from `Endpoint::parse("127.0.0.1:0")?` | refused |
| `listen()` refuses `` `.max_concurrent_streams(Count::Max(0))` would let no HTTP/2 stream open`` with the hint | hyper, `h2c(true)` | refused |
| `listen()` refuses `max_inflight(Count::Max(0))` with the hint | `Embedded` | refused |

Against known violations, each one planted alone into `crates/` and the scratch crate rerun on
stable and on 1.88, with identical results on both:

| Plant | Fails |
| --- | --- |
| `Router::build` refuses `Duration::MAX` instead of `Duration::ZERO` | both route checks (`listen()` succeeds) |
| `zero_timeout_tier` always answers "on the handler" | the `#[routes]` impl check |
| `Router::build` refuses any declared zero, through `meta_all` | the `#[routes]` impl check, whose report also names `ZeroTimeoutOnImpl::overrides` |
| `keep_alive` stores `Some(Duration::ZERO)` | both keep-alive checks: 1 sleep requested while waiting, 2 after the event |
| `prepare_app` does not call `check_zero_counts` | all three `Count::Max(0)` checks (`listen()` succeeds) |
| `check_zero_counts` checks `max_inflight` only | the `max_concurrent_streams` check |
| `config.rs` defines its own `enum Count { Default, Max(u32), Unlimited }` | the crate does not compile: 8 `E0308`, each "expected `ulo_transport::Count`, found `ulo_http::config::Count`" |

The last plant replaces the `#[doc(no_inline)]` line with the enum as well, since rustc 1.88
rejects that attribute on anything but a `use`. No check outside its item failed under any plant.
Each file was restored by `cp` from a saved copy and touched, and `cmp` confirmed it
byte-identical before the final runs.

## Fourth round: the eighteenth response

`transports/RESPONSE.md` "Eighteenth response" accepted third-round decisions 1, 2 and 5. On 3,
`Timeout` wraps a `Bound`, so a handler can lift the timeout its `#[routes]` impl declares, and the
zero refusal takes the same hint as the other settings. On 4, the refusal still reads the timeout
each route runs with, and reports once per zero declaration.

Files changed: `crates/ulo-http/src/{limits.rs, router/mod.rs}`.

### The signatures

```rust
// ulo_http
pub struct Timeout(pub Bound);                                   // was Timeout(pub Duration); Clone, Copy, Debug, PartialEq, Eq

impl Timeout {
    pub const OFF: Timeout;                                      // Timeout(Bound::Unbounded)
    pub const fn after(d: Duration) -> Timeout;                  // Timeout(Bound::After(d))
}
```

### What each item does

1. **`Timeout(Bound)`.** `Bound::After(d)` is a timeout of `d`. `Bound::Unbounded` and
   `Bound::Default` are no timeout: the server has no route-timeout default, and `Default` names
   that default. The conversion happens in one place, the crate-private `Timeout::duration`, which
   `Router::build` applies to the most specific declaration, `info.meta::<Timeout>()`. The service
   reads the route's `Option<Duration>` as before, so a handler's `#[meta(Timeout::OFF)]` replaces
   its impl's `Timeout::after(..)` and the route runs untimed. The `Timeout` doc states the rule,
   both constructors and what `Bound::Default` means.
2. **The hint.** The zero refusal ends "write `Timeout::OFF` to turn the timeout off".
3. **One failure per declaration.** `Router::build` collects each zero it finds under its
   declaration, keyed by the controller and, for a handler's own declaration, the handler. After
   the table is walked, each declaration becomes one `Configure` failure listing every route that
   runs with it: `` `#[meta(Timeout(..))]` on the `#[routes]` impl of `ZeroFive` would cancel every
   request on `GET /a`, `POST /b`, `GET /c/{id}`, `GET /d`, `GET /e`; write `Timeout::OFF` to turn
   the timeout off ``, and for a handler `` `#[meta(Timeout(..))]` on the handler `Api::slow` would
   cancel every request on `GET /slow`; write `Timeout::OFF` to turn the timeout off ``. The text
   writes `Timeout(..)` for either spelling of the zero, `Timeout::after(Duration::ZERO)` or
   `Timeout(Bound::After(Duration::ZERO))`, and drops the third round's "before the handler could
   answer", as the response's example does. A zero that every handler overrides reaches no route
   and is not refused. The controller is named through `Failure::naming`, so it prints by its last
   path segment unless another type in the report prints alike.

### Decisions for sign-off

#### 1. The routes are listed in declaration order

- **Written:** the order the controllers mounted the handlers, which within one `#[routes]` impl is
  the order of the methods in the source.
- **Why:** the failure asks for an edit to that impl, and a reader checks the list against it top
  to bottom. Precedence order is the router's matching order, which a reader of one impl has no
  reason to know, and `Router::build` sorts by it only after every failure is collected.
- **Alternative:** precedence order, most specific pattern first.

#### 2. A handler's `Timeout(Bound::Default)` is the server's default, not its impl's timeout

- **Written:** under an impl declaring `Timeout::after(Duration::from_millis(50))`, a handler
  declaring `Timeout(Bound::Default)` runs with no timeout.
- **Why:** the handler's declaration is the most specific, so it wins, and `Bound::Default` names
  the server's route-timeout default, as it names the app's default for a hook or a construction.
  That default is none today.
- **Alternative:** read `Bound::Default` as no declaration, so the handler inherits the impl's
  timeout. That would make `Default` mean "inherit" here and "the app's default" everywhere else.

#### 3. The grouped failures follow the route table's other failures

- **Written:** a duplicate route, a parameter-name clash or a `Path<T>` mismatch is reported in the
  order found. The zero-timeout failures come after all of them, in the order each declaration's
  first route was found.
- **Why:** a declaration's failure lists routes found later in the walk, so it is complete only
  once the walk ends.
- **Alternative:** hold a place for each declaration at its first route and fill it after the walk.

#### 4. The routes are written in backticks

- **Written:** `` `GET /a`, `POST /b` ``.
- **Why:** the route table's other failures write a route that way (``both answer `GET /a` ``).
  The response's example writes them bare.
- **Alternative:** bare, `GET /a, POST /b`.

### Divergences

1. **DESIGN §3.6 writes `#[meta(Timeout(..))]` and does not say that it wraps a `Bound`, that a
   handler can lift an inherited timeout, or that the zero refusal is grouped by declaration.**
   Each is in the rustdoc of `Timeout`. DESIGN.md is not edited here.
2. **Third-round divergence 1 and second-round divergences 1 to 3 stand.**

### Not covered

- The core's zero-timeout check (F313).

### Verification

- `cargo check --workspace --all-targets` and `cargo +1.88 check --workspace --all-targets` pass.
  The warning lists (cargo JSON, deduplicated by file, line and message) are identical to HEAD's,
  17 entries on each toolchain, all in `crates/ulo/src`. HEAD's lists came from a `git archive`
  export of `fa4cd490`. The comparison reported a planted unused import in
  `crates/ulo-http/src/limits.rs` as one new entry, and matched HEAD again once it was removed.
- `cargo test --workspace` passes: 20 test binaries, 3 passed, 18 ignored, none failed, as at HEAD.
- `RUSTDOCFLAGS="-D warnings" cargo doc -p ulo-http -p ulo-http-hyper --no-deps` passes.
- The scratch crate `embed` (`#![deny(warnings)]`) keeps its 98 checks and adds 8. All 106 pass on
  rustc stable and on 1.88, with identical labels. Round 12's route timeouts are now written
  `Timeout::after(..)` on the handlers and `Timeout(Bound::After(Duration::ZERO))` on the impl, and
  its two zero checks expect the grouped text and the new hint. The new checks, all on `Embedded`
  with `ulo_tokio::Timer` and each handler sleeping 200 ms where a timeout is tested:

| Check | Declarations | Result |
| --- | --- | --- |
| `Timeout::after(d)` evaluates in a `const` and equals `Timeout(Bound::After(d))`; `Timeout::OFF` equals `Timeout(Bound::Unbounded)` | — | as stated |
| A handler's `Timeout::OFF` lifts its impl's timeout | impl `Timeout::after(50 ms)`, handler `Timeout::OFF` | `200` |
| Its sibling without the override | impl `Timeout::after(50 ms)` | `504` |
| A handler's `Timeout(Bound::Default)` under the impl's timeout | impl `Timeout::after(50 ms)`, handler `Timeout(Bound::Default)` | `200` |
| `Timeout(Bound::Default)` on an impl | impl `Timeout(Bound::Default)` | `200` |
| One failure listing `` `GET /a`, `POST /b`, `GET /c/{id}`, `GET /d`, `GET /e` `` in declaration order, the hint once, no `problems:`, the overriding route not named | impl `Timeout::after(ZERO)`, five inheriting routes, a sixth with `Timeout::OFF` between `/d` and `/e` | refused |
| Not refused | impl `Timeout::after(ZERO)`, one handler `Timeout::after(5 s)`, one `Timeout::OFF` | listens |
| `2 problems:`, naming `embed::r13a::ZeroApi` (impl) and `embed::r13b::ZeroApi::one` (handler) | two controllers named `ZeroApi`, one zero on each | refused |

Against known violations, each one planted alone into `crates/` and the scratch crate rerun on
stable and on 1.88, with identical results on both:

| Plant | Fails |
| --- | --- |
| `Router::build` takes the first declaration carrying a duration, through `meta_all`, so a handler can tighten but not lift | the `Timeout::OFF` check and the handler's `Bound::Default` check (both `504`) |
| `Timeout::duration` reads `Bound::Default` as 50 ms | both `Bound::Default` checks (`504`) |
| `Timeout::duration` reads `Bound::After(_)` as no timeout | the sibling check (`200`) |
| Each zero route opens its own declaration | the five-route check (`5 problems:`) |
| The failure keeps the third-round hint | both round-12 zero checks and the five-route check |
| The impl tier prints the controller with `Display` instead of `Names::of` | the short-name check (`ZeroApi` printed short) |
| `Router::build` refuses any declared zero, through `meta_all` | round 12's impl check, the five-route check (each naming the overriding route) and the all-overridden check (refused) |
| `declared_on_method` always answers `false` | round 12's handler check and the short-name check (a handler's zero reported "on the `#[routes]` impl") |
| `Timeout::after` is not `const` | the crate does not compile: `E0015`, "cannot call non-const associated function `ulo_http::Timeout::after` in constants" |

No check outside its item failed under any plant. Each file was restored by `cp` from a saved copy
and touched, and `cmp` confirmed it byte-identical before the final runs.
