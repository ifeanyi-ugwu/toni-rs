# Divergences: race 2a's pre-compile batch, core and `ulo-net`

The user-visible choices made building T21 and T13 (`transports/RESPONSE.md`, "Fourth response"),
each with what the decision, the brief or the design says, what was written, and why. All await
sign-off.

Files: `crates/ulo/src/{error/wiring,graph/mod,graph/wire,transport/inputs,lib}.rs`,
`crates/ulo-net/{Cargo.toml,src/activation.rs}`, the root `Cargo.toml`. No cargo was run.

## 1. `InputOrigin`'s shape (T21)

- **Decision:** `InputConflict { key, first: InputOrigin, second: InputOrigin }`, `InputOrigin` public
  and `#[non_exhaustive]`, with a variant for each side CX renders: declared by a transport, declared
  in a module with a seeder, bound in a module.
- **Written:** in `error/wiring.rs`, re-exported as `ulo::InputOrigin`:

  ```rust
  #[non_exhaustive]
  #[derive(Clone, Debug)]
  pub enum InputOrigin {
      Transport { name: &'static str, at: &'static Location<'static> },
      Module { module: ModuleName, seeders: Vec<&'static str>, at: &'static Location<'static> },
      Binding { module: ModuleName, at: &'static Location<'static> },
  }
  ```

  `name` is the transport as reports name it (`Http`). Each seeder is the full `type_name`, as
  `InputNotSeeded::seeder` already carries it, and the line prints its last path segment. `Display`
  writes CX's line text unchanged: ``declared by transport `Http` at <file>:<line>``,
  ``declared in AppModule with seeder `Rpc` at <file>:<line>``, ``bound in AppModule at
  <file>:<line>``. `{:#}` writes the module name with its full path. Derives are `Clone` and `Debug`
  only.
- **`seeders`, not the brief's `seeder`:** DESIGN §10.2 gives the module side as
  `seeders: Vec<&'static str>`, following T15's answer (an input is declared once, `seeded_by` once
  per seeder). The field is public, and a singular field would break when T15 is built. The core
  still records one seeder per module declaration, so every `seeders` built today holds one entry and
  prints CX's singular text. Several print as ``with seeders `Ws`, `WsConnect` ``. Reverting to the
  brief's `seeder: &'static str` touches the variant, `line`, and the two `vec![..]` sites in
  `wire.rs`.
- **`first` is always `Transport`:** every conflict is found from a transport's declaration, which
  sits in `first`. `second` is the module's declaration, the binding, or the later of two transports.
  The variant's doc says so.
- **`Binding` over `Bound`:** `ulo::Bound` already names the timer vocabulary.

## 2. One type for the graph's record and for the report (T21)

- **Spine 19:** a crate-private `InputOrigin { Module(ModuleId), Transport { name, at } }` on
  `InputDecl::origin`.
- **Written:** the public type replaces it, as transports DESIGN's X4 row puts it ("`InputOrigin` on
  the declaration and on both sides of `InputConflict`"). The freeze records a module's declaration as
  `Module { module, seeders, at }`, its `ModuleName` and location being in hand there.
  `InputDecl::origin` never holds `Binding`, stated on `InputDecl`. `input_site` is gone; the conflict
  code clones the declaration's origin.
- **Why:** the internal enum had one reader, `input_site`, which only ever saw its `Transport` arm.
  Two enums for one fact would need a conversion and keep a `ModuleId` arm nothing printed.

## 3. A module name in a conflict lengthens on collision (T21)

- **Before:** each side was text, so two modules printing alike stayed alike, which was T21's stated
  cost.
- **Written:** `WiringError::module_names` returns the modules both origins name, and the report
  renders them through the collision rule every other entry uses. `InputOrigin`'s own `Display`
  cannot see the rest of the report and writes the short form unless asked for `{:#}`.

## 4. The manifests (T13)

- **Root:** `serde_urlencoded` removed from `[workspace.dependencies]` and `libc = "0.2"` added. No
  member's manifest or source names `serde_urlencoded`, and neither does any excluded crate's
  manifest. `ulo-cli`, excluded, names `libc` directly and is unaffected.
- **`ulo-net`:** `libc` from the workspace under `[target.'cfg(unix)'.dependencies]`, and `tracing`
  from the workspace for the fallback's log (entry 7).

## 5. macOS does not serve `SO_ACCEPTCONN`; `TCP_CONNECTION_INFO` decides (T13)

- **Decision:** read `SO_ACCEPTCONN` through `getsockopt` with `libc` where socket2's `is_listener` is
  unavailable, macOS and the other BSDs.
- **Probed (this machine, Darwin 27.0.0, xnu-13432):** `getsockopt(SOL_SOCKET, SO_ACCEPTCONN)` fails
  with `ENOPROTOOPT` on an unbound, a bound and a listening TCP socket, while `SO_TYPE` and
  `SO_REUSEADDR` read on the same socket. The header defines the constant and the kernel does not
  serve it. Python's `socket.getsockopt` and a C program built against the SDK headers agree. The
  decision's read fails on every macOS start, whatever the failure is mapped to.
- **Written:** on `target_vendor = "apple"`, `getsockopt(IPPROTO_TCP, TCP_CONNECTION_INFO)`, compared
  with `TCPS_LISTEN` (1, from `<netinet/tcp_fsm.h>`; libc does not export it). Probed: `0`
  (`TCPS_CLOSED`) when unbound or bound, `1` when listening, `4` when connected, on IPv4 and IPv6.
- **A libc layout mismatch:** `libc::tcp_connection_info` is 176 bytes and the kernel's struct is 112,
  libc spelling the C bitfields after `tcpi_rttvar` as whole `u32`s. The kernel fills a prefix of the
  larger buffer (probed: 112 bytes written into 176), and only `tcpi_state`, at offset 0, is read. A
  comment at the call says so.
- **Unsafe:** two blocks, each with a SAFETY comment: `mem::zeroed` for the struct (integers and
  `MaybeUninit` padding only), and the `getsockopt` call (buffers of the stated sizes that outlive
  the call; the descriptor is `vet`'s borrow).

## 6. The other BSDs read `SO_ACCEPTCONN`, unprobed (T13)

- **Written:** on DragonFly, NetBSD and OpenBSD, `getsockopt(SOL_SOCKET, SO_ACCEPTCONN)`. libc defines
  the constant on all three.
- **Unverified:** whether those kernels serve it. No such machine was available, and entry 5 shows a
  defined constant is not evidence.
- **Hedge:** `ENOPROTOOPT` from the read falls back to the bound-port check with its log (entry 7),
  rather than refusing every inherited socket. Any other error refuses the descriptor, as the socket2
  arm's `unwrap_or(false)` does. The Apple arm follows the same rule.

## 7. The fallback and its log (T13)

- **Decision:** where neither works, keep the fallback and log it.
- **Written:** every Unix target outside socket2's five, Apple and the three BSDs keeps the
  nonzero-bound-port check; Solaris, illumos, Haiku and Cygwin are among them. When the check accepts
  a descriptor it logs at `warn`: ``inherited descriptor #0 is accepted on its bound port 8080: this
  platform reports no listen state, so a socket that never called `listen` fails at its first accept
  instead``. A port of 0 refuses without a log, and `NotListening` reports it. `vet` takes the
  descriptor's index for the message, in the numbering `NotListening` uses.
- **Why `warn`:** the check behind a startup refusal is weaker than designed, and `ulo-http` logs a
  degraded answer at `warn`. It fires once per inherited socket per process, under socket activation
  only.
- **Left out:** Solaris and illumos have `SO_ACCEPTCONN` in libc and could join entry 6's arm, but
  neither was probed. socket2 0.6.4 and 0.6.5 read it on Cygwin, but the workspace requires only
  `socket2 = "0.6"` and the earlier 0.6 releases were not on disk to check, so Cygwin stays out of the
  socket2 arm.

## 8. Tests (T13)

- **Written:** two tests in `activation.rs` passing a real loopback socket through `vet`. A socket
  that is only bound is refused, on the targets with a listen-state read. A listening socket is
  accepted, on every Unix target. The first fails against the replaced fallback by construction: the
  bound socket holds a nonzero ephemeral port.
- **Not run:** no cargo in this batch. `cargo test -p ulo-net` on this machine exercises the Apple arm.

## Design text this leaves stale

- `transports/DIVERGENCES.md` T13 has macOS reading `SO_ACCEPTCONN` through `libc`. macOS reads
  `TCP_CONNECTION_INFO` instead (entry 5). Its reading section's "(BSD fallback: a nonzero bound
  port, T13)" describes the replaced check.
- `race2a-N.md` entry 2's platform gap and request R1 make the same `SO_ACCEPTCONN` claim for macOS.
- `transports/DIVERGENCES.md` T21's "Built" paragraph and `race2a-CX.md` entry 7 describe the `String`
  sides.
