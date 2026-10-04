//! `ulo __exec -- <child> <args>`: the trampoline `ulo dev` runs each child through (transports
//! DESIGN §9). `LISTEN_PID` must equal the child's process ID, and between fork and exec only
//! async-signal-safe calls are allowed, which `setenv` is not. The trampoline runs after an exec,
//! with an ordinary allocator, sets `LISTEN_PID` to its own pid and execs the child, which keeps
//! the pid. Hidden from `--help`.

use std::ffi::OsString;

use anyhow::Result;

#[derive(clap::Args)]
pub struct ExecArgs {
    /// The child and its arguments
    #[arg(last = true, required = true)]
    command: Vec<OsString>,
}

/// Execs the child with `LISTEN_PID` set to this process's ID. Returns only on failure.
pub fn execute(args: ExecArgs) -> Result<()> {
    let _ = args;
    todo!()
}
