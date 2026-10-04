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
