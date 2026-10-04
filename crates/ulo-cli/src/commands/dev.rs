//! `ulo dev`: watch, rebuild, restart (transports DESIGN §9). With `--listen ADDR` the command binds
//! the address itself and passes the socket to each child as descriptor 3 through the
//! socket-activation protocol, `LISTEN_FDS`, `LISTEN_FDNAMES` naming the address and `LISTEN_PID`
//! set by the `ulo __exec` trampoline, with `ULO_DEV=1`, so `EndpointSpec::resolve` in the child
//! resolves an `Endpoint::Addr` whose text equals the name to the inherited socket. UDP is outside
//! `--listen`. Socket holding is Unix-only.

use anyhow::Result;

/// The descriptor the child reads the passed socket from, per the socket-activation protocol.
#[cfg(unix)]
const LISTEN_FD: std::os::fd::RawFd = 3;

/// Put `source_fd` at [`LISTEN_FD`] so it survives the coming exec.
///
/// Runs between fork and exec, so everything it calls must be async-signal-safe; `dup2` and
/// `fcntl` are.
///
/// # Safety
///
/// Call only in that window, where the process is single-threaded and no allocation or locking
/// may happen.
#[cfg(unix)]
unsafe fn place_listen_fd(source_fd: std::os::fd::RawFd) -> std::io::Result<()> {
    if source_fd == LISTEN_FD {
        // dup2 does nothing when both descriptors are equal, and in particular leaves FD_CLOEXEC
        // set, which would close the socket at exec. Clear it directly instead.
        let flags = unsafe { libc::fcntl(LISTEN_FD, libc::F_GETFD) };
        if flags == -1 || unsafe { libc::fcntl(LISTEN_FD, libc::F_SETFD, flags & !libc::FD_CLOEXEC) } == -1 {
            return Err(std::io::Error::last_os_error());
        }
    } else if unsafe { libc::dup2(source_fd, LISTEN_FD) } == -1 {
        return Err(std::io::Error::last_os_error());
    }
    // The descriptor now carries no FD_CLOEXEC, so it survives this exec and the trampoline's.
    Ok(())
}

#[derive(clap::Args)]
pub struct DevArgs {
    /// Build and run in release mode
    #[arg(long)]
    release: bool,

    /// Package to run, for a workspace with more than one
    #[arg(short = 'p', long, value_name = "SPEC")]
    package: Option<String>,

    /// Run this binary instead of the package default
    #[arg(long, value_name = "NAME", conflicts_with = "example")]
    bin: Option<String>,

    /// Run this example instead of a binary
    #[arg(long, value_name = "NAME")]
    example: Option<String>,

    /// Features to activate; repeat the flag or comma-separate the list
    #[arg(short = 'F', long, value_name = "FEATURES")]
    features: Vec<String>,

    /// Activate every feature of every selected package
    #[arg(long)]
    all_features: bool,

    /// Do not activate the `default` feature
    #[arg(long)]
    no_default_features: bool,

    /// A cargo flag this command does not mirror, forwarded as given. Repeat once per argument:
    /// `--cargo-arg --timings --cargo-arg --offline`.
    #[arg(long, value_name = "ARG", allow_hyphen_values = true)]
    cargo_arg: Vec<String>,

    /// Hold a listening socket on this address across restarts and pass it to every child, named
    /// by the address, so an application written `Server::new(ADDR)` adopts it under `ULO_DEV=1`
    #[arg(long, value_name = "ADDR")]
    listen: Option<String>,

    /// Arguments passed to the application binary (after `--`)
    #[arg(last = true)]
    args: Vec<String>,
}

pub async fn execute(args: DevArgs) -> Result<()> {
    let _ = args;
    #[cfg(unix)]
    let _ = place_listen_fd;
    todo!()
}
