# Divergences: race 2a, N (networking and runtime)

Every place `ulo-net` and `ulo-tokio` depart from `transports/DESIGN.md` §2.7, §8 and §9, or fill
what they leave open, beyond the spine's entries 42–46. Each entry gives what the design says, what
was written, and why. Requests and notes for other areas follow the entries.

Files: `crates/ulo-net/src/{activation,listener,tls}.rs`, `crates/ulo-tokio/src/lib.rs`. No public
signature changed. `endpoint.rs`, both manifests and `spawn_in` were left as the spine wrote them.

## Entries

### 1. How the socket-activation variables are read

- **Design:** §2.7: an inherited socket is accepted only when `LISTEN_PID` equals the process ID;
  §12: a `LISTEN_PID` mismatch fails startup at `prepare`. Silent on absent or malformed variables.
- **Written:** `sd_listen_fds_with_names`' rules, with two departures.
  - No `LISTEN_PID`, or no `LISTEN_FDS`, is no sockets, which is not an error.
  - A `LISTEN_PID` naming another process is `PidMismatch`. `sd_listen_fds` answers zero sockets
    there; §12 asks for the error.
  - `LISTEN_FDS=0` is no sockets, where `sd_listen_fds` reports `EINVAL`. A count above
    `i32::MAX - 3` is `Malformed`.
  - `LISTEN_FDNAMES`, when set, must hold one colon-separated name per descriptor, or it is
    `Malformed`. When unset, the descriptors carry no name and answer by index only.
  - A variable that is not UTF-8 is `Malformed`.

### 2. The listening-TCP check (the point §"Points each area resolves" leaves to N)

- **Design:** silent on how an inherited descriptor is checked.
- **Written:** `FD_CLOEXEC` is set on every descriptor first, whatever it turns out to be. A
  descriptor then passes when `SO_TYPE` is `SOCK_STREAM`, its local address is IPv4 or IPv6, and
  `SO_ACCEPTCONN` is set, read through socket2's `is_listener`.
- **Platform gap:** socket2 reads `SO_ACCEPTCONN` on Linux, Android, FreeBSD, Fuchsia and AIX only.
  On macOS and the other Unix targets a nonzero bound port stands in for it. A bound socket that
  never called `listen` passes there and fails at its first accept in the serve loop. Request R1
  closes the gap.
- **Ownership:** every descriptor is checked through a `BorrowedFd` before any is adopted as an
  `OwnedFd`. A `LISTEN_FDS` that overstates the count reaches a descriptor some other part of the
  process owns, such as the runtime's event queue, and the check refuses it without closing it.

### 3. One descriptor that fails the check fails the whole activation

- **Design:** silent.
- **Written:** `Activation::get()` answers `NotListening { index }` naming the first failing
  descriptor, and no descriptor is adopted.
- **Why:** `prepare` asks only `contains`, a `bool` that cannot carry a reason; failing in `get`
  lets `prepare` report the index rather than "no inherited socket is named ..".
- **Consequence for 2b:** a supervisor that passes a UDP socket beside TCP listeners fails
  activation for every server. 2b's UDP link needs the slots to hold a socket of either kind.

### 4. Several descriptors under one name

- **Design:** `Endpoint::inherited("http")` is "matched by `LISTEN_FDNAMES`"; silent on a name
  carried by more than one descriptor.
- **Written:** a name answers for every descriptor carrying it, and `take` hands over the first not
  yet taken. systemd names every socket of a unit after the unit by default, so a unit listening on
  IPv4 and IPv6 passes two descriptors named alike; a server lists the endpoint twice to get both.
- **Consequence:** `contains` answers per endpoint. Two endpoints naming a socket that exists once
  both pass `prepare`, and the second fails at `bind` with `Missing`, a `StartupError::Bind` rather
  than `Configure`. Note P2 covers it.

### 5. Socket activation off Unix (the point left to N)

- **Design:** §9: socket holding is Unix-only; a Windows handoff is the path to add later.
- **Written:** `Activation::get()` answers `Unsupported` on every non-Unix target, whatever the
  environment holds, so an `Endpoint::inherited(..)` fails `prepare` there with "socket activation
  is supported on Unix only". `Endpoint::Addr` is unaffected.

### 6. What `Tls::load` builds

- **Design:** rustls; a bad certificate, a mismatched key or an unreadable file fails at `prepare`;
  ALPN per transport.
- **Written:** the `ring` provider passed explicitly through `ServerConfig::builder_with_provider`,
  rustls's safe default protocol versions (TLS 1.2 and 1.3), no client authentication, one
  certificate chain. The key match is rustls's own check of the key's public half against the leaf
  certificate. A PEM error from a file names the file.
- **Why the explicit provider:** `ServerConfig::builder()` uses the process default, which panics
  when the build enables both rustls backends and nothing installed one.
- **Consequence for 2b:** `Tls` has no client-certificate verification, which gRPC's mTLS guard
  (§6.1) needs.

### 7. When `shutdown_signal` installs its handlers, and what a second signal does

- **Design:** §8: SIGINT and SIGTERM on Unix, Ctrl-C and Ctrl-Close on Windows. Silent on when the
  handlers are installed.
- **Written:** the handlers are installed at the future's first poll, inside the runtime polling
  it, so calling `shutdown_signal()` outside a runtime does not panic. A signal arriving between
  the call and the first poll takes the OS default action. A handler that fails to install is
  skipped; with none installed, or on a target that is neither Unix nor Windows, the future never
  resolves.
- **Behaviour the user sees:** on Unix tokio keeps a handler installed for the rest of the
  process, so a second SIGINT during the shutdown sequence does not end it. The doc comment says
  so.

## Requests

### R1. `libc` for `SO_ACCEPTCONN` on macOS (coordinator, optional)

- **File:** root `Cargo.toml`, a workspace entry `libc = "0.2"`; then `crates/ulo-net/Cargo.toml`.
- **Why:** socket2 does not read `SO_ACCEPTCONN` on macOS or the other BSDs, and std reads no
  socket option (entry 2). `libc` is already in the lockfile through socket2. Without it the
  listening check on those targets stays the bound-port check.

## Notes for other areas

- **P1 (`ulo-http/src/server.rs`):** call `Activation::get()` only when an endpoint is
  `Endpoint::Inherited`. The variables stay in the environment (§2.7), so a process spawned by a
  socket-activated app sees a `LISTEN_PID` that is not its own and answers `PidMismatch`, which
  would fail a `prepare` that binds addresses only.
- **P2:** two endpoints naming one inherited socket pass `contains` each (entry 4). Counting the
  endpoints per name in `prepare` turns the `bind` failure into a `Configure` one.
- **P3:** `Tls::load` takes `&[&[u8]]`. Byte-string literals of different lengths do not coerce
  in one array literal; write `&[b"h2".as_slice(), b"http/1.1".as_slice()]`.
- **2b:** entry 3 (UDP inherited sockets) and entry 6 (client certificates for gRPC).

**P's request N1, applied by the orchestrating session after N finished:** `Activation::count(&ListenerName)
-> usize`, the number of inherited sockets not yet taken that a name answers for, so `prepare` can refuse a
name listed more often than it was inherited (P2). Additive; no signature changed.
