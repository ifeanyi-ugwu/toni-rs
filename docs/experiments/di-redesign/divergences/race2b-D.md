# Divergences: race 2b, area D (the development command)

Every place area D departs from `transports/DESIGN.md` §9 and §2.7, the twentieth response (Q17,
Q18), `REVIEW_2B.md` (R40–R42) or the spine's surface, or fills what they leave open. Each entry
gives what the design says, what was written, and why. All await the user's sign-off.

Files written: `crates/ulo-cli/Cargo.toml`, `crates/ulo-cli/src/main.rs`,
`crates/ulo-cli/src/commands/dev.rs`, `crates/ulo-cli/src/commands/exec.rs`,
`crates/ulo-net/src/endpoint.rs`. No `todo!()` remains in them. No cargo was run.

## The surface, as written

```rust
// ulo-net (EndpointSpec::resolve keeps its signature; one item added, entry 6)
impl EndpointSpec {
    pub fn resolve(&self) -> Result<Endpoint, EndpointError>;            // + the ULO_DEV lookup
    pub fn resolve_as_written(&self) -> Result<Endpoint, EndpointError>; // new: no lookup
}

// ulo-cli: `ulo dev [DevArgs]` keeps the spine's flags; `ulo __exec -- <child> <args>` is hidden.
```

A child of `ulo dev --listen 0.0.0.0:8080` runs as `ulo __exec -- target/debug/app <args>` with
`ULO_DEV=1`, `LISTEN_FDS=1`, `LISTEN_FDNAMES=0.0.0.0%3A8080` and the held socket at descriptor 3;
the trampoline adds `LISTEN_PID=<its pid>` and execs the binary.

## The resolution (`ulo-net`)

### 1. The socket's name is the address with `:` and `%` escaped

- **Design (§9):** the command passes the socket "with the address text as the socket's name".
- **Written:** the name is the address's canonical text with `%` written `%25` and `:` written
  `%3A`: `0.0.0.0:8080` is named `0.0.0.0%3A8080`, `[::1]:8080` is `[%3A%3A1]%3A8080`. `dev.rs`
  writes it and `endpoint.rs` derives the same name from the application's endpoint, each with a
  comment naming the other.
- **Why:** `LISTEN_FDNAMES` separates names with `:`, and systemd refuses `:` inside a name. With
  the text verbatim, `ulo-net`'s activation splits `0.0.0.0:8080` into two names for one
  descriptor and fails every inherited endpoint as `Malformed { variable: "LISTEN_FDNAMES" }`.
  Escaping `%` as well keeps the encoding injective, since an IPv6 address's text carries
  `%<scope>`. The alternative, reading `LISTEN_FDNAMES` as one name when `LISTEN_FDS=1`, departs from
  the protocol and needs a change in `activation.rs`, which D does not own.

### 2. The address is compared as parsed, not as written

- **Design (§9):** an `Endpoint::Addr` "whose text equals an inherited socket's name".
- **Written:** the name is derived from the parsed `SocketAddr`'s `Display`, so
  `Server::new("0.0.0.0:8080")`, `Server::new(SocketAddr)` and `Endpoint::Addr(..)` all match, and
  `[0:0:0:0:0:0:0:0]:8080` matches `--listen [::]:8080`.
- **Why:** an `EndpointSpec` built from a `SocketAddr` or an `Endpoint` has no text, and two
  spellings of one address bind the same socket.

### 3. An activation that cannot be read falls back to the address

- **Design:** "without the variable, or with no name matching, the address binds as written, and
  the `LISTEN_PID` check keeps a stale variable harmless"; silent on a malformed activation.
- **Written:** under `ULO_DEV=1` the lookup calls `Activation::get()`; any error resolves the
  address as written. `PidMismatch` and `Unsupported` log at `debug`, every other error at `warn`.
  `ULO_DEV` counts only when it is exactly `1`.
- **Why:** `PidMismatch` is the stale case the design names (a process the app spawns inherits
  `ULO_DEV` too). A malformed variable or a descriptor that is not listening means the supervisor
  passed something wrong; the bind that follows fails on the held address, and the `warn` says
  why. `EndpointError` has one message, "not a socket address", so `resolve` cannot report it.

### 4. An endpoint written as inherited is untouched

`Endpoint::inherited(..)` resolves as in production (§9), and `Endpoint::Addr` resolution happens
only inside `resolve`, which servers call in `prepare`. HTTP's `check_inherited` then counts the
resolved name like any other: two endpoints written as the held address in one server are refused
as "listed 2 times, and 1 inherited socket answers to it"; across two servers the second fails at
`bind` with `Missing`. A backend declaring `inherited_sockets: false` fails `prepare` on an address
`ulo dev` holds; the hyper backend declares `true`.

### 5. `--listen` on port 0

`ulo dev --listen 0.0.0.0:0` holds a socket on a port the OS chooses, and an endpoint written
`0.0.0.0:0` adopts it, so the port stays the same across restarts. The banner prints the chosen
port beside the text the endpoint must be written as. Two servers both written `0.0.0.0:0` then
compete for the one socket (entry 4); nothing refuses `--listen` on port 0 for that.

### 6. `EndpointSpec::resolve_as_written` is new

- **Plan (rule 5, and "Owned by D"):** `EndpointSpec::resolve`'s signature is frozen; adding a public
  item is a divergence.
- **Written:** `resolve_as_written`, the old body of `resolve`, which `resolve` now calls before
  the lookup.
- **Why:** two callers must not take the lookup, and `resolve` has no parameter to say so. A client
  connects to the address it names; under `resolve` a client in a `ulo dev` child whose address
  equals the held one receives an `Endpoint::Inherited` it cannot connect to. A UDP link's socket is
  outside `--listen` (Q18); under `resolve` a UDP endpoint written as the held TCP address resolves
  to the inherited TCP socket, which `Udp`'s `prepare` refuses. See the requests below.

## The command (`ulo-cli`)

### 7. `cargo build`, then the binary, replaces `cargo run`

- **Design (§9):** "rebuilds (`cargo build`)", then signals the old child and starts the new one on
  the same sockets. `master` restarted `cargo run`, which stops the application before building.
- **Written:** each change runs `cargo build --message-format=json-render-diagnostics` with the
  mirrored flags while the old child serves; only a build that succeeds stops it and starts the new
  binary. A failed build leaves the application running. Diagnostics reach the terminal on stderr,
  rendered by cargo; stdout carries the JSON the command reads.
- **What replaced `master`:** `DevArgs::cargo_args()` (a `cargo run` line ending in `-- <args>`) is
  `build_args()`, without the application's arguments, which go to the child directly.

### 8. Which binary a build runs

- **Design:** silent.
- **Written:** the `compiler-artifact` messages' `executable` paths, chosen as `cargo run` chooses:
  the `--bin` or `--example` named; else the only binary built; else the one a package names in
  `default-run`, read from `cargo metadata --no-deps --format-version 1` and matched by package ID;
  else an error listing the binaries and naming `--bin`. A binary whose path stays the same keeps
  its job and is restarted in it; a different path stops the old job and starts a new one.

### 9. A change during a build

- **Design:** silent.
- **Written:** the build runs to completion, and when a change arrived meanwhile it builds again
  before replacing the application, so a binary behind the source never starts. Ctrl-C during a
  build drops it, which kills cargo (`kill_on_drop`); the terminal's SIGINT has reached cargo
  already, being in the command's process group.

### 10. `watchexec` is kept over bare `notify`

- **Design (§9):** "watches the source (`notify`)". The plan leaves the choice to D.
- **Written:** `watchexec` 8.2 for the watch, its globset filterer with the project's ignore files,
  `**/target/**` and `**/.git/**` excluded, `rs` and `toml` only, a 250 ms throttle, as on `master`;
  and its re-exported supervisor (`watchexec::job::start_job`) for the child: grouped spawn, process
  groups on Unix and Job Objects on Windows, SIGTERM with a grace, the spawn hook. The build runs on
  `tokio::process`, and a supervisor task sequences build, stop and start; the action handler only
  forwards changes and the stop signal to it.
- **Why:** `watchexec` wraps `notify`, and the user directed `master`'s command to wrap it rather
  than rebuild a watcher. The design's restart order (build while serving, then stop, then start)
  needs the build outside the job the child runs in, which `master`'s one-job form could not give.
- **Dropped:** the `watchexec-events` dependency and the seeded startup event `master` sent: the
  supervisor builds on start without waiting for an event.

### 11. The grace before SIGKILL stays at two seconds

- **Design (§9):** "Signal the old child, which drains gracefully"; silent on a bound.
- **Written:** `STOP_GRACE` of 2 s, carried from `master`, for a restart and for Ctrl-C.
- **Why, and the cost:** an application's own drain window defaults to 10 s; a WebSocket or SSE
  client open across a save would hold every restart that long, with requests queueing in the
  backlog meanwhile. A drain longer than 2 s is cut by SIGKILL. `DevArgs` carries no knob, since
  the spine froze its flags; a `--stop-grace` flag is the change if a longer drain is wanted.

### 12. What runs between fork and exec

- **Design (§9):** "nothing runs between fork and exec but the `dup2` and `fcntl`".
- **Written:** `place_listen_fd`, carried with its body, as the only hook the command installs. The
  grouped spawn adds `setpgid`, and std's spawn resets the signal mask and `SIGPIPE`; both are
  async-signal-safe and both ran on `master`.
- **Why:** the process group is what lets the stop reach anything the application spawned, and
  what keeps the terminal's Ctrl-C at the command, which then stops the child in order.

### 13. The trampoline runs only when a socket is held

- **Design (§9):** the command re-executes its own binary as `ulo __exec -- <child> <args>`, in the
  context of `--listen`.
- **Written:** with `--listen` each child is `current_exe() __exec -- <binary> <args>`; without it,
  the binary is started directly. A binary path that is not UTF-8 is refused when a socket is held,
  since `watchexec`'s `Program::Exec` takes its arguments as `String`s.

### 14. The child's environment

- **Design (§9):** `ULO_DEV=1`, "which the command sets"; silent on whether it depends on `--listen`.
- **Written:** every child gets `ULO_DEV=1` and has `LISTEN_PID` removed (the trampoline sets it). With
  a held socket it gets `LISTEN_FDS=1` and `LISTEN_FDNAMES=<name>`; without one both are removed,
  so a `ulo dev` itself started under socket activation passes nothing of its own to the child.
  `master`'s `LISTEN_FDS_FIRST_FD`, a `listenfd` extension, is gone: `ulo-net` reads from descriptor
  3, as the protocol fixes.

### 15. What `--listen` accepts (the plan's open point)

- **Written:** an IP socket address in the form `Endpoint::parse` reads, optionally prefixed
  `tcp://`. Refused, each with its own message: a bare port (`master` read `8080` as
  `127.0.0.1:8080`), `udp://…` (the UDP refusal of §12), and anything else that is not a socket
  address, a host name included.
- **Why:** the held socket goes to the endpoint written as the same address, and a shorthand
  expanding to `127.0.0.1` misses the common `0.0.0.0:8080`. The application then binds
  `0.0.0.0:8080` itself and fails with "address in use", since the command holds the port. The
  command cannot see which transport an endpoint belongs to, so the UDP refusal is the scheme the
  user writes; a UDP link at the held address binds its own socket once it resolves with
  `resolve_as_written` (request 2).

### 16. `--listen` off Unix is ignored with a warning

- **Design (§9):** "On Windows the command restarts without holding sockets, so a client can see a
  refused connection during a restart."
- **Written:** `--listen` is accepted, a warning says the socket is not held and why, and the
  command runs without it; the application binds the address itself, which `ULO_DEV` leaves as
  written there, since the activation is unsupported. `master` refused the flag.

### 17. `__exec` and `main`

- **Plan's open point (how `__exec` stays out of `--help`):** `#[command(name = "__exec", hide =
  true)]` on the `Exec` variant, behind the `dev` feature.
- **Written:** `main` is a plain `fn` that dispatches `__exec` before any runtime exists, and builds
  a `tokio` runtime for every other command (`master` used `#[tokio::main]`). The trampoline then
  runs on one thread, sets `LISTEN_PID` through `std::process::Command::env`, and execs with
  `CommandExt::exec`; it never calls the `unsafe` `std::env::set_var`. `run` handles `Exec` too,
  which `main` never reaches, so the match stays exhaustive without a panic.

### 18. A child that exits is not reported

As on `master`: the supervisor learns nothing when the application exits on its own; the next
successful build starts it again.

## Requests for other areas

1. **R, `crates/ulo-rpc-tcp/src/link.rs`:** resolve the endpoint with
   `EndpointSpec::resolve_as_written` when the link connects as a client, and with `resolve` when it
   listens. Otherwise a TCP client in a `ulo dev` child whose address equals the held socket's
   resolves to an inherited socket it cannot connect to (entry 6).
2. **R, `crates/ulo-rpc-udp/src/link.rs`:** resolve with `resolve_as_written` on both sides. UDP is
   outside `--listen`, and under `resolve` a UDP endpoint written as the held TCP address becomes
   an `Endpoint::Inherited`, which the link's `prepare` refuses (entries 6, 15).
3. **Coordinator, `crates/ulo-net/src/activation.rs` (no 2b owner):** `Activation::get`'s doc says
   to call it only for an `Endpoint::Inherited`. `EndpointSpec::resolve` now calls it for an
   `Endpoint::Addr` under `ULO_DEV=1` and treats `PidMismatch` as no sockets (entry 3); the doc
   should name that caller.
4. **W, G:** nothing to change. The standalone WebSocket server and the gRPC server resolve with
   `resolve`, which is what lets `ulo dev --listen` hold their sockets too.

## Owed after the race

- `master`'s `socket_handoff` tests for `place_listen_fd` are not carried (rule 2); they apply
  unchanged. `master`'s `cargo_invocation` tests target `cargo_args()`; against `build_args()` they
  lose the `--` cases, since the application's arguments no longer pass through cargo.
- Tests the new code needs: `dev_socket_name` against `socket_name` (one encoding, two copies);
  `resolve` under `ULO_DEV=1` with and without a matching held socket and under a stale
  `LISTEN_PID`; `listen_addr`'s refusals; `pick`'s choice over several binaries and `default-run`.
- `commands/new.rs`, `commands/generate.rs` and `templates/` are untouched and target the old API.
- The documentation pages for `ulo dev` state UDP's exclusion, the Windows behaviour, the
  two-second grace and the `--listen` forms of entry 15.

## Not verified

Nothing was compiled or run. In particular: `Watchexec::new`'s closure, `Job::set_spawn_hook`'s
closure and `CommandWrap::command_mut` (read from `watchexec` 8.2.0, `watchexec-supervisor` 5.2.0
and `process-wrap` 9.1.0 in the registry); the `error @ (A { .. } | B)` binding in `held_by_dev`;
`break 'session` from inside `tokio::select!`, whose expansion holds no loop (read from tokio
1.53); cargo's artifact `package_id` matching `cargo metadata`'s `id`; and the end-to-end handoff,
`ulo dev` through the trampoline into an app's `prepare`, which only a run on Unix shows.
