//! `ulo __exec -- <child> <args>`: the trampoline `ulo dev` starts each child through when it holds
//! a socket (transports DESIGN §9). `LISTEN_PID` must equal the child's process ID, and between
//! fork and exec only async-signal-safe calls are allowed, which `setenv` is not. The trampoline
//! runs after an exec, with an ordinary allocator, sets `LISTEN_PID` to its own pid and execs the
//! child, which keeps the pid. Descriptor 3 reaches the child because `ulo dev` cleared its
//! `FD_CLOEXEC` and nothing here opens or closes a descriptor. Hidden from `--help`.

use std::ffi::OsString;

use anyhow::{Result, bail};

#[derive(clap::Args)]
pub struct ExecArgs {
    /// The child and its arguments
    #[arg(last = true, required = true)]
    command: Vec<OsString>,
}

/// Execs the child with `LISTEN_PID` set to this process's ID. Returns only on failure.
pub fn execute(args: ExecArgs) -> Result<()> {
    let Some((child, child_args)) = args.command.split_first() else {
        bail!("`ulo __exec` needs the command to run after `--`");
    };
    exec(child, child_args)
}

#[cfg(unix)]
fn exec(child: &OsString, args: &[OsString]) -> Result<()> {
    use std::os::unix::process::CommandExt;

    // The environment is built here, before the exec replaces the process, which is why the
    // variable can be set at all: no fork precedes it.
    let error = std::process::Command::new(child)
        .args(args)
        .env("LISTEN_PID", std::process::id().to_string())
        .exec();
    Err(anyhow::Error::new(error).context(format!("cannot exec `{}`", std::path::Path::new(child).display())))
}

#[cfg(not(unix))]
fn exec(_child: &OsString, _args: &[OsString]) -> Result<()> {
    bail!("`ulo __exec` hands inherited sockets to its child, which needs Unix")
}
