use anyhow::{Context, Result, anyhow};
use colored::*;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use watchexec::Watchexec;
use watchexec::command::{Command, Program, SpawnOptions};
use watchexec_events::{Event, Priority};
use watchexec_filterer_globset::GlobsetFilterer;
use watchexec_signals::Signal;

/// SIGTERM-to-SIGKILL grace when stopping the app. Generated apps have no
/// signal handlers and exit immediately; apps that wire shutdown_handle()
/// get this long to drain.
const STOP_GRACE: Duration = Duration::from_secs(2);

/// Where the passed socket lands in the child, per the systemd socket-activation
/// convention that `listenfd` reads.
#[cfg(unix)]
const LISTEN_FD: std::os::fd::RawFd = 3;

/// Bind the socket the supervisor keeps across restarts.
///
/// A bare port is accepted as shorthand for localhost.
#[cfg(unix)]
fn bind_held_socket(spec: &str) -> Result<std::net::TcpListener> {
    let addr = if spec.contains(':') {
        spec.to_string()
    } else {
        format!("127.0.0.1:{spec}")
    };
    std::net::TcpListener::bind(&addr).with_context(|| format!("Failed to bind {addr}"))
}

/// Hand the held socket to every process this job spawns.
///
/// `LISTEN_PID` stays unset on purpose: the socket is claimed by the
/// application, which `cargo run` spawns as a grandchild, so no pid announced
/// here could match the process that reads it. `listenfd` skips the pid check
/// when the variable is absent.
#[cfg(unix)]
fn install_socket_hook(job: &watchexec::job::Job, socket: Arc<std::net::TcpListener>) {
    use std::os::fd::AsRawFd;

    job.set_spawn_hook(move |command, _| {
        // Keep the listener owned by the closure: the child inherits a
        // duplicate, and the original must outlive every restart.
        let source_fd = socket.as_raw_fd();
        let command = command.command_mut();
        command
            .env("LISTEN_FDS", "1")
            .env("LISTEN_FDS_FIRST_FD", LISTEN_FD.to_string())
            .env_remove("LISTEN_PID");

        // SAFETY: dup2 and fcntl are async-signal-safe, which is the
        // constraint on anything running between fork and exec.
        unsafe {
            command.pre_exec(move || {
                if source_fd == LISTEN_FD {
                    // dup2 does nothing when both descriptors are equal, and
                    // in particular leaves FD_CLOEXEC set — which would close
                    // the socket at exec. Clear it directly instead.
                    let flags = libc::fcntl(LISTEN_FD, libc::F_GETFD);
                    if flags == -1
                        || libc::fcntl(LISTEN_FD, libc::F_SETFD, flags & !libc::FD_CLOEXEC) == -1
                    {
                        return Err(std::io::Error::last_os_error());
                    }
                } else if libc::dup2(source_fd, LISTEN_FD) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                // The duplicate carries no FD_CLOEXEC, so it survives this
                // exec and the one cargo does for the application binary.
                Ok(())
            });
        }
    });
}

#[derive(clap::Args)]
pub struct DevArgs {
    /// Build and run in release mode
    #[arg(long)]
    release: bool,

    /// Hold the listening socket here and pass it to every restart, so
    /// requests arriving mid-rebuild queue instead of being refused. The
    /// application must adopt the inherited socket (see the socket_activation
    /// example); one that binds its own will fail with "address in use".
    #[arg(long, value_name = "ADDR")]
    listen: Option<String>,

    /// Arguments passed to the application binary (after `--`)
    #[arg(last = true)]
    args: Vec<String>,
}

pub async fn execute(args: DevArgs) -> Result<()> {
    let project_root = std::env::current_dir().context("Failed to read current directory")?;
    if !project_root.join("Cargo.toml").exists() {
        return Err(anyhow!(
            "No Cargo.toml in {} — run `toni dev` from the project root",
            project_root.display()
        ));
    }

    let mut cargo_args = vec!["run".to_string()];
    if args.release {
        cargo_args.push("--release".to_string());
    }
    if !args.args.is_empty() {
        cargo_args.push("--".to_string());
        cargo_args.extend(args.args.iter().cloned());
    }
    let display_command = format!("cargo {}", cargo_args.join(" "));

    // grouped: cargo run spawns the app as a grandchild; signalling the
    // process group is the only way the stop reaches it.
    let command = Arc::new(Command {
        program: Program::Exec {
            prog: "cargo".into(),
            args: cargo_args,
        },
        options: SpawnOptions {
            grouped: true,
            ..Default::default()
        },
    });

    let (ignore_files, ignore_errors) = ignore_files::from_origin(project_root.as_path()).await;
    for err in ignore_errors {
        eprintln!("{}", format!("[toni dev] ignore file: {err}").yellow());
    }

    let filterer = GlobsetFilterer::new(
        &project_root,
        std::iter::empty::<(String, Option<PathBuf>)>(),
        [
            ("**/target/**".to_string(), None),
            ("**/.git/**".to_string(), None),
        ],
        std::iter::empty::<PathBuf>(),
        ignore_files,
        ["rs", "toml"].map(OsString::from),
    )
    .await
    .context("Failed to build the file filter")?;

    let held_socket = match args.listen.as_deref() {
        None => None,
        #[cfg(unix)]
        Some(spec) => Some(Arc::new(bind_held_socket(spec)?)),
        #[cfg(not(unix))]
        Some(_) => {
            return Err(anyhow!(
                "--listen needs Unix file-descriptor passing and is not supported on this platform"
            ));
        }
    };

    let job_id = watchexec::Id::default();
    let hook_socket = held_socket.clone();
    let wx = Watchexec::new(move |mut action| {
        if action
            .signals()
            .any(|sig| matches!(sig, Signal::Interrupt | Signal::Terminate))
        {
            eprintln!("{}", "[toni dev] stopping".dimmed());
            action.quit_gracefully(Signal::Terminate, STOP_GRACE);
            return action;
        }

        if action.paths().next().is_some() {
            eprintln!("{}", "[toni dev] change detected, restarting".dimmed());
        }
        let is_new = action.get_job(job_id).is_none();
        let job = action.get_or_create_job(job_id, || command.clone());

        #[cfg(unix)]
        if let Some(socket) = hook_socket.clone().filter(|_| is_new) {
            install_socket_hook(&job, socket);
        }
        #[cfg(not(unix))]
        let _ = is_new;

        job.restart_with_signal(Signal::Terminate, STOP_GRACE);
        action
    })
    .map_err(|e| anyhow!(e))?;

    wx.config.pathset([project_root.clone()]);
    wx.config.throttle(Duration::from_millis(250));
    wx.config.filterer(filterer);
    wx.config.on_error(|hook| {
        eprintln!("{}", format!("[toni dev] {}", hook.error).yellow());
    });

    eprintln!(
        "{}",
        format!(
            "[toni dev] watching {} — running `{}` (Ctrl-C to stop)",
            project_root.display(),
            display_command
        )
        .green()
    );
    if let Some(socket) = &held_socket {
        let addr = socket
            .local_addr()
            .map(|a| a.to_string())
            .unwrap_or_else(|_| "?".to_string());
        eprintln!(
            "{}",
            format!("[toni dev] holding {addr} — passing it to each restart").green()
        );
    }

    // The action handler only runs on events; seed one so the app starts
    // before the first file change.
    wx.send_event(Event::default(), Priority::Urgent)
        .await
        .map_err(|e| anyhow!(e))?;

    wx.main()
        .await
        .context("Watcher task panicked")?
        .map_err(|e| anyhow!(e))?;
    Ok(())
}
