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

/// Put `source_fd` at [`LISTEN_FD`] so it survives the coming exec.
///
/// Runs between fork and exec, so everything it calls must be
/// async-signal-safe; `dup2` and `fcntl` are.
///
/// # Safety
///
/// Call only in that window, where the process is single-threaded and no
/// allocation or locking may happen.
#[cfg(unix)]
unsafe fn place_listen_fd(source_fd: std::os::fd::RawFd) -> std::io::Result<()> {
    if source_fd == LISTEN_FD {
        // dup2 does nothing when both descriptors are equal, and in particular
        // leaves FD_CLOEXEC set — which would close the socket at exec. Clear
        // it directly instead.
        let flags = unsafe { libc::fcntl(LISTEN_FD, libc::F_GETFD) };
        if flags == -1
            || unsafe { libc::fcntl(LISTEN_FD, libc::F_SETFD, flags & !libc::FD_CLOEXEC) } == -1
        {
            return Err(std::io::Error::last_os_error());
        }
    } else if unsafe { libc::dup2(source_fd, LISTEN_FD) } == -1 {
        return Err(std::io::Error::last_os_error());
    }
    // The descriptor now carries no FD_CLOEXEC, so it survives this exec and
    // the one cargo does for the application binary.
    Ok(())
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

        // SAFETY: pre_exec runs its closure in exactly the window
        // place_listen_fd requires.
        unsafe {
            command.pre_exec(move || place_listen_fd(source_fd));
        }
    });
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

    /// A cargo flag this command does not mirror, forwarded as given. Repeat
    /// once per argument: `--cargo-arg --timings --cargo-arg --offline`.
    #[arg(long, value_name = "ARG", allow_hyphen_values = true)]
    cargo_arg: Vec<String>,

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

impl DevArgs {
    /// Assemble the cargo invocation the watcher restarts.
    ///
    /// Cargo stops reading flags at the `--`, so everything selecting what to
    /// build has to precede it and everything after it belongs to the
    /// application binary.
    fn cargo_args(&self) -> Vec<String> {
        let mut cargo_args = vec!["run".to_string()];
        if self.release {
            cargo_args.push("--release".to_string());
        }
        for (flag, selection) in [
            ("--package", &self.package),
            ("--bin", &self.bin),
            ("--example", &self.example),
        ] {
            if let Some(name) = selection {
                cargo_args.push(flag.to_string());
                cargo_args.push(name.clone());
            }
        }
        for features in &self.features {
            cargo_args.push("--features".to_string());
            cargo_args.push(features.clone());
        }
        if self.all_features {
            cargo_args.push("--all-features".to_string());
        }
        if self.no_default_features {
            cargo_args.push("--no-default-features".to_string());
        }
        cargo_args.extend(self.cargo_arg.iter().cloned());
        if !self.args.is_empty() {
            cargo_args.push("--".to_string());
            cargo_args.extend(self.args.iter().cloned());
        }
        cargo_args
    }
}

pub async fn execute(args: DevArgs) -> Result<()> {
    let project_root = std::env::current_dir().context("Failed to read current directory")?;
    if !project_root.join("Cargo.toml").exists() {
        return Err(anyhow!(
            "No Cargo.toml in {} — run `toni dev` from the project root",
            project_root.display()
        ));
    }

    let cargo_args = args.cargo_args();
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

    // Annotated because the arm that produces a socket is compiled out on
    // platforms without descriptor passing.
    let held_socket: Option<Arc<std::net::TcpListener>> = match args.listen.as_deref() {
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
    #[cfg(unix)]
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

#[cfg(test)]
mod cargo_invocation {
    use super::DevArgs;
    use clap::Parser;

    /// `DevArgs` is a flattened subcommand, so a parser has to host it before
    /// the flag surface can be exercised.
    #[derive(Parser)]
    struct Harness {
        #[command(flatten)]
        dev: DevArgs,
    }

    fn cargo_args(flags: &[&str]) -> Vec<String> {
        let argv = std::iter::once("toni-dev").chain(flags.iter().copied());
        Harness::try_parse_from(argv)
            .expect("flags should parse")
            .dev
            .cargo_args()
    }

    #[test]
    fn no_flags_run_the_package_default() {
        assert_eq!(cargo_args(&[]), ["run"]);
    }

    #[test]
    fn target_selection_reaches_cargo() {
        assert_eq!(
            cargo_args(&["-p", "my-app", "--bin", "server"]),
            ["run", "--package", "my-app", "--bin", "server"]
        );
    }

    #[test]
    fn feature_selection_reaches_cargo() {
        assert_eq!(
            cargo_args(&[
                "-F",
                "tls",
                "--features",
                "metrics",
                "--no-default-features"
            ]),
            [
                "run",
                "--features",
                "tls",
                "--features",
                "metrics",
                "--no-default-features"
            ]
        );
    }

    /// Cargo stops reading its own flags at the `--`, so a cargo flag placed
    /// after it reaches the application binary instead — where an unknown
    /// argument is the application's error to report, not cargo's.
    #[test]
    fn cargo_flags_precede_the_app_separator() {
        let args = cargo_args(&["--example", "demo", "--", "--config", "local.toml"]);

        assert_eq!(
            args,
            ["run", "--example", "demo", "--", "--config", "local.toml"]
        );
    }

    #[test]
    fn unmirrored_flags_pass_through_verbatim() {
        assert_eq!(
            cargo_args(&["--cargo-arg", "--timings", "--cargo-arg", "--offline"]),
            ["run", "--timings", "--offline"]
        );
    }

    /// Cargo refuses the pair too, but only after the watcher has started and
    /// on every restart after that.
    #[test]
    fn refuses_a_binary_and_an_example_together() {
        assert!(
            Harness::try_parse_from(["toni-dev", "--bin", "server", "--example", "demo"]).is_err(),
            "a run has one target"
        );
    }
}

#[cfg(all(test, unix))]
mod socket_handoff {
    use super::{LISTEN_FD, place_listen_fd};
    use std::net::TcpListener;
    use std::os::fd::{AsRawFd, RawFd};
    use std::sync::Mutex;

    /// Descriptor 3 is process-wide, so the cases cannot run concurrently.
    static FD_SLOT: Mutex<()> = Mutex::new(());

    fn cloexec_set(fd: RawFd) -> bool {
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        assert_ne!(flags, -1, "fd {fd} is not open");
        flags & libc::FD_CLOEXEC != 0
    }

    /// Port `fd` is bound to, or `None` when it is closed or not a socket.
    ///
    /// Comparing this against the original listener identifies the socket,
    /// which a "is it a socket" check alone would not.
    fn socket_port(fd: RawFd) -> Option<u16> {
        let mut addr: libc::sockaddr_in = unsafe { std::mem::zeroed() };
        let mut len = std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;
        let rc =
            unsafe { libc::getsockname(fd, &mut addr as *mut _ as *mut libc::sockaddr, &mut len) };
        (rc == 0).then(|| u16::from_be(addr.sin_port))
    }

    /// Run `body` with descriptor 3 restored afterwards, so moving a socket
    /// into it cannot disturb the test harness.
    fn with_listen_fd_slot<T>(body: impl FnOnce() -> T) -> T {
        let _guard = FD_SLOT.lock().unwrap_or_else(|e| e.into_inner());
        let saved = unsafe { libc::fcntl(LISTEN_FD, libc::F_DUPFD_CLOEXEC, 30) };
        let out = body();
        unsafe {
            if saved >= 0 {
                libc::dup2(saved, LISTEN_FD);
                libc::close(saved);
            } else {
                libc::close(LISTEN_FD);
            }
        }
        out
    }

    #[test]
    fn places_the_socket_where_the_child_reads_it() {
        with_listen_fd_slot(|| {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            // Hold the source well away from the target slot so the branch
            // under test is the copying one.
            let source = unsafe { libc::fcntl(listener.as_raw_fd(), libc::F_DUPFD, 20) };
            assert!(source >= 20, "could not place the source descriptor");

            unsafe { place_listen_fd(source) }.unwrap();

            assert_eq!(socket_port(LISTEN_FD), Some(port), "not the held socket");
            assert!(
                !cloexec_set(LISTEN_FD),
                "a descriptor marked close-on-exec never reaches the application"
            );
            unsafe { libc::close(source) };
        });
    }

    /// `dup2` returns success without doing anything when both descriptors are
    /// equal, so the close-on-exec flag survives and the socket dies at exec.
    /// Whether the held socket lands on descriptor 3 depends on how many the
    /// watcher already holds, which makes this silent whenever it happens.
    #[test]
    fn clears_close_on_exec_when_the_socket_already_occupies_the_slot() {
        with_listen_fd_slot(|| {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            assert_ne!(
                unsafe { libc::dup2(listener.as_raw_fd(), LISTEN_FD) },
                -1,
                "could not move the socket into the slot"
            );
            // Restore the close-on-exec flag dup2 just cleared, so the socket
            // sits in the slot exactly as one bound in this process would.
            let flags = unsafe { libc::fcntl(LISTEN_FD, libc::F_GETFD) };
            unsafe { libc::fcntl(LISTEN_FD, libc::F_SETFD, flags | libc::FD_CLOEXEC) };
            assert!(cloexec_set(LISTEN_FD), "precondition");

            unsafe { place_listen_fd(LISTEN_FD) }.unwrap();

            assert_eq!(socket_port(LISTEN_FD), Some(port), "the socket was lost");
            assert!(!cloexec_set(LISTEN_FD));
        });
    }
}
