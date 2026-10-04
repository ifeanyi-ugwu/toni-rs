//! `ulo dev`: watch, rebuild, restart (transports DESIGN §9).
//!
//! Each change rebuilds with `cargo build` while the running application keeps serving; only a
//! build that succeeds stops it, with SIGTERM and a grace period, and starts the new binary. A
//! failed build leaves the running application in place.
//!
//! With `--listen ADDR` the command binds the address itself and passes the socket to each child
//! as descriptor 3 through the socket-activation protocol: `LISTEN_FDS=1`, `LISTEN_FDNAMES` naming
//! the address, and `LISTEN_PID` set by the `ulo __exec` trampoline the child is started through.
//! Every child runs under `ULO_DEV=1`, under which `EndpointSpec::resolve` in the child resolves an
//! `Endpoint::Addr` equal to the held address to the inherited socket, so the application's `main`
//! is the same in development and production. While no child accepts, connections queue in the
//! held socket's backlog instead of being refused.
//!
//! UDP is outside `--listen`: the activation adopts listening TCP sockets alone, so a UDP link
//! binds its address itself and restarts without holding it. Socket holding is Unix-only; on other
//! platforms `--listen` is ignored with a warning and a client can see a refused connection during
//! a restart.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use colored::Colorize;
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use watchexec::Watchexec;
use watchexec::command::{Command, Program, SpawnOptions};
use watchexec::job::{Job, start_job};
use watchexec_filterer_globset::GlobsetFilterer;
use watchexec_signals::Signal;

/// SIGTERM-to-SIGKILL grace when stopping the application. An application past it is killed
/// mid-drain, which a development restart accepts in exchange for not waiting out the app's own
/// drain window on every save.
const STOP_GRACE: Duration = Duration::from_secs(2);

/// Read by `ulo-net`'s `EndpointSpec::resolve`.
const DEV_VAR: &str = "ULO_DEV";

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

    /// Hold a listening TCP socket on this address across restarts and pass it to every child,
    /// so connections arriving mid-restart queue instead of being refused. An endpoint the
    /// application writes as the same address (`Server::new("0.0.0.0:8080")` for
    /// `--listen 0.0.0.0:8080`) adopts it. UDP is not supported. Unix only.
    #[arg(long, value_name = "ADDR")]
    listen: Option<String>,

    /// Arguments passed to the application binary (after `--`)
    #[arg(last = true)]
    args: Vec<String>,
}

impl DevArgs {
    /// The `cargo build` invocation each change runs. The application's arguments are not cargo's:
    /// they go to the child.
    fn build_args(&self) -> Vec<String> {
        let mut args = vec!["build".to_string(), "--message-format=json-render-diagnostics".to_string()];
        if self.release {
            args.push("--release".to_string());
        }
        for (flag, selection) in [("--package", &self.package), ("--bin", &self.bin), ("--example", &self.example)] {
            if let Some(name) = selection {
                args.push(flag.to_string());
                args.push(name.clone());
            }
        }
        for features in &self.features {
            args.push("--features".to_string());
            args.push(features.clone());
        }
        if self.all_features {
            args.push("--all-features".to_string());
        }
        if self.no_default_features {
            args.push("--no-default-features".to_string());
        }
        args.extend(self.cargo_arg.iter().cloned());
        args
    }
}

/// The socket `--listen` holds for the whole session. Every child receives a duplicate at
/// descriptor 3; this one stays open across restarts, which is what keeps the backlog accepting.
#[cfg_attr(not(unix), allow(dead_code))]
struct Held {
    listener: std::net::TcpListener,
    addr: SocketAddr,
    name: String,
}

/// What every build and every start reads.
struct Plan {
    root: PathBuf,
    build_args: Vec<String>,
    bin: Option<String>,
    example: Option<String>,
    app_args: Vec<String>,
    held: Option<Arc<Held>>,
    /// This binary, which starts each child as `ulo __exec -- <child>` when a socket is held.
    trampoline: Option<PathBuf>,
}

enum Trigger {
    Change,
    Quit,
}

pub async fn execute(args: DevArgs) -> Result<()> {
    let root = std::env::current_dir().context("cannot read the current directory")?;
    if !root.join("Cargo.toml").exists() {
        bail!("no Cargo.toml in {}; run `ulo dev` from the project root", root.display());
    }

    let held = match args.listen.as_deref() {
        Some(spec) => hold(listen_addr(spec)?)?,
        None => None,
    };
    let trampoline = match held {
        Some(_) => Some(std::env::current_exe().context("cannot locate the `ulo` binary for the `__exec` trampoline")?),
        None => None,
    };
    let build_args = args.build_args();
    let display_command = format!("cargo {}", build_args.join(" "));
    let plan = Arc::new(Plan {
        root: root.clone(),
        build_args,
        bin: args.bin,
        example: args.example,
        app_args: args.args,
        held,
        trampoline,
    });

    let (ignore_files, ignore_errors) = ignore_files::from_origin(root.as_path()).await;
    for error in ignore_errors {
        eprintln!("{}", format!("[ulo dev] ignore file: {error}").yellow());
    }
    let filterer = GlobsetFilterer::new(
        &root,
        std::iter::empty::<(String, Option<PathBuf>)>(),
        [("**/target/**".to_string(), None), ("**/.git/**".to_string(), None)],
        std::iter::empty::<PathBuf>(),
        ignore_files,
        ["rs", "toml"].map(std::ffi::OsString::from),
    )
    .await
    .context("cannot build the file filter")?;

    let (triggers, received) = mpsc::unbounded_channel();
    let handler_triggers = triggers.clone();
    let wx = Watchexec::new(move |mut action| {
        if action.signals().any(|signal| matches!(signal, Signal::Interrupt | Signal::Terminate)) {
            let _ = handler_triggers.send(Trigger::Quit);
            action.quit();
        } else if action.paths().next().is_some() {
            let _ = handler_triggers.send(Trigger::Change);
        }
        action
    })
    .map_err(|error| anyhow!(error))?;
    wx.config.pathset([root.clone()]);
    wx.config.throttle(Duration::from_millis(250));
    wx.config.filterer(filterer);
    wx.config.on_error(|hook| {
        eprintln!("{}", format!("[ulo dev] {}", hook.error).yellow());
    });

    eprintln!(
        "{}",
        format!("[ulo dev] watching {}; building with `{display_command}` (Ctrl-C to stop)", root.display()).green()
    );
    if let Some(held) = &plan.held {
        let local = held.listener.local_addr().map_or_else(|_| held.addr.to_string(), |addr| addr.to_string());
        eprintln!(
            "{}",
            format!("[ulo dev] holding {local} across restarts; an endpoint written `{}` adopts it", held.addr).green()
        );
    }

    // The supervisor builds and starts the application without waiting for a first change.
    let supervisor = tokio::spawn(supervise(plan, received));
    let watched = wx.main().await;
    // A watcher that ended on an error rather than a signal still owes the child its stop.
    let _ = triggers.send(Trigger::Quit);
    supervisor.await.context("the supervisor task panicked")?;
    watched.context("the watcher task panicked")?.map_err(|error| anyhow!(error))?;
    Ok(())
}

/// What `--listen` takes: an IP socket address, written as the application writes its endpoint,
/// with an optional `tcp://` in front.
fn listen_addr(spec: &str) -> Result<SocketAddr> {
    if let Some(addr) = spec.strip_prefix("udp://") {
        bail!(
            "`--listen {spec}`: UDP is outside `--listen`; the application adopts listening TCP sockets alone, so a UDP link binds `{addr}` itself and restarts without holding it"
        );
    }
    let text = spec.strip_prefix("tcp://").unwrap_or(spec);
    if text.parse::<u16>().is_ok() {
        bail!(
            "`--listen {spec}` names a port alone; write the address the application listens on, such as `0.0.0.0:{text}` or `127.0.0.1:{text}`, since the held socket goes to the endpoint written as that address"
        );
    }
    text.parse::<SocketAddr>()
        .map_err(|_| anyhow!("`--listen {spec}` is not a socket address such as `0.0.0.0:8080`"))
}

#[cfg(unix)]
fn hold(addr: SocketAddr) -> Result<Option<Arc<Held>>> {
    let listener = std::net::TcpListener::bind(addr).with_context(|| format!("cannot listen on {addr}"))?;
    Ok(Some(Arc::new(Held { listener, addr, name: socket_name(addr) })))
}

#[cfg(not(unix))]
fn hold(addr: SocketAddr) -> Result<Option<Arc<Held>>> {
    eprintln!(
        "{}",
        format!(
            "[ulo dev] --listen {addr} is ignored: holding a socket across restarts needs Unix descriptor passing, so the application binds {addr} itself and a client can see a refused connection during a restart"
        )
        .yellow()
    );
    Ok(None)
}

/// The `LISTEN_FDNAMES` name of the socket held on `addr`: the address's text with `%` written
/// `%25` and `:` written `%3A`, since `LISTEN_FDNAMES` separates names with `:`. `ulo-net`'s
/// `EndpointSpec::resolve` derives the same name from the application's endpoint.
#[cfg(unix)]
fn socket_name(addr: SocketAddr) -> String {
    let text = addr.to_string();
    let mut name = String::with_capacity(text.len() + 2);
    for c in text.chars() {
        match c {
            '%' => name.push_str("%25"),
            ':' => name.push_str("%3A"),
            c => name.push(c),
        }
    }
    name
}

/// The application as currently started, and the binary it was started from.
struct Running {
    job: Job,
    task: JoinHandle<()>,
    executable: PathBuf,
}

enum Built {
    Binary(PathBuf),
    /// Cargo reported the failure on stderr already.
    Failed,
}

/// Builds on start and after every change, and replaces the running application once a build
/// succeeds. Returns on `Trigger::Quit` after stopping the application.
async fn supervise(plan: Arc<Plan>, mut triggers: mpsc::UnboundedReceiver<Trigger>) {
    let mut running: Option<Running> = None;
    let mut stale = true;
    'session: loop {
        if stale {
            stale = false;
            eprintln!("{}", "[ulo dev] building".dimmed());
            let outcome = {
                let build = build(&plan);
                tokio::pin!(build);
                loop {
                    tokio::select! {
                        outcome = &mut build => break outcome,
                        trigger = triggers.recv() => match trigger {
                            Some(Trigger::Change) => stale = true,
                            // Dropping the build future kills cargo.
                            Some(Trigger::Quit) | None => break 'session,
                        },
                    }
                }
            };
            // A change during the build leaves the binary behind the source: build again before
            // replacing the application.
            if stale {
                continue;
            }
            match outcome {
                Ok(Built::Binary(executable)) => {
                    if let Err(error) = start(&plan, &mut running, executable).await {
                        eprintln!("{}", format!("[ulo dev] {error:#}").red());
                    }
                }
                Ok(Built::Failed) if running.is_some() => {
                    eprintln!("{}", "[ulo dev] build failed; the running application keeps serving".red());
                }
                Ok(Built::Failed) => eprintln!("{}", "[ulo dev] build failed; waiting for a change".red()),
                Err(error) => eprintln!("{}", format!("[ulo dev] {error:#}").red()),
            }
        }
        match triggers.recv().await {
            Some(Trigger::Change) => {
                eprintln!("{}", "[ulo dev] change detected".dimmed());
                stale = true;
            }
            Some(Trigger::Quit) | None => break,
        }
    }
    eprintln!("{}", "[ulo dev] stopping".dimmed());
    if let Some(running) = running {
        stop(running).await;
    }
}

/// One `cargo build`, and the binary it produced for this session's selection.
async fn build(plan: &Plan) -> Result<Built> {
    let mut cargo = tokio::process::Command::new("cargo");
    cargo
        .args(&plan.build_args)
        .current_dir(&plan.root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true);
    let mut child = cargo.spawn().context("cannot start `cargo build`")?;
    let stdout = child.stdout.take().ok_or_else(|| anyhow!("`cargo build` has no output to read"))?;

    let mut artifacts = Vec::new();
    let mut lines = BufReader::new(stdout).lines();
    while let Some(line) = lines.next_line().await.context("cannot read `cargo build`'s output")? {
        // Lines that are not artifact messages, or not JSON at all, carry nothing this reads.
        let Ok(message) = serde_json::from_str::<Message>(&line) else { continue };
        if message.reason != "compiler-artifact" {
            continue;
        }
        if let (Some(package_id), Some(target), Some(executable)) = (message.package_id, message.target, message.executable) {
            artifacts.push(Artifact { package_id, kind: target.kind, name: target.name, executable });
        }
    }
    let status = child.wait().await.context("cannot wait for `cargo build`")?;
    if !status.success() {
        return Ok(Built::Failed);
    }
    pick(plan, artifacts).await.map(Built::Binary)
}

/// A line of `cargo build --message-format=json-render-diagnostics`.
#[derive(Deserialize)]
struct Message {
    reason: String,
    #[serde(default)]
    package_id: Option<String>,
    #[serde(default)]
    target: Option<Target>,
    #[serde(default)]
    executable: Option<PathBuf>,
}

#[derive(Deserialize)]
struct Target {
    kind: Vec<String>,
    name: String,
}

struct Artifact {
    package_id: String,
    kind: Vec<String>,
    name: String,
    executable: PathBuf,
}

impl Artifact {
    fn is(&self, kind: &str) -> bool {
        self.kind.iter().any(|k| k == kind)
    }
}

/// The binary `cargo run` would run for the same selection: the named binary or example, the
/// only binary built, or the one a package names in `default-run`.
async fn pick(plan: &Plan, artifacts: Vec<Artifact>) -> Result<PathBuf> {
    let named = match (&plan.example, &plan.bin) {
        (Some(name), _) => Some(("example", name)),
        (None, Some(name)) => Some(("bin", name)),
        (None, None) => None,
    };
    if let Some((kind, name)) = named {
        return artifacts
            .into_iter()
            .find(|artifact| artifact.is(kind) && artifact.name == *name)
            .map(|artifact| artifact.executable)
            .ok_or_else(|| anyhow!("`cargo build` reported no {kind} named `{name}`"));
    }

    let bins: Vec<Artifact> = artifacts.into_iter().filter(|artifact| artifact.is("bin")).collect();
    if let [only] = bins.as_slice() {
        return Ok(only.executable.clone());
    }
    if bins.is_empty() {
        bail!("`cargo build` produced no binary to run; select one with `--bin` or `--example`");
    }
    let defaults = default_runs(&plan.root).await?;
    let mut chosen = bins
        .iter()
        .filter(|artifact| defaults.iter().any(|(id, name)| *id == artifact.package_id && *name == artifact.name));
    match (chosen.next(), chosen.next()) {
        (Some(artifact), None) => Ok(artifact.executable.clone()),
        _ => {
            let names: Vec<&str> = bins.iter().map(|artifact| artifact.name.as_str()).collect();
            bail!(
                "`cargo build` produced {} binaries ({}); choose one with `--bin`, or name it in the package's `default-run`",
                bins.len(),
                names.join(", ")
            )
        }
    }
}

/// Each package's `default-run`, keyed by the package ID cargo's artifact messages carry.
async fn default_runs(root: &Path) -> Result<Vec<(String, String)>> {
    let output = tokio::process::Command::new("cargo")
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(root)
        .stdin(Stdio::null())
        .stderr(Stdio::inherit())
        .output()
        .await
        .context("cannot run `cargo metadata`")?;
    if !output.status.success() {
        bail!("`cargo metadata` failed");
    }
    let metadata: Metadata = serde_json::from_slice(&output.stdout).context("cannot read `cargo metadata`'s output")?;
    Ok(metadata
        .packages
        .into_iter()
        .filter_map(|package| package.default_run.map(|name| (package.id, name)))
        .collect())
}

#[derive(Deserialize)]
struct Metadata {
    packages: Vec<MetadataPackage>,
}

#[derive(Deserialize)]
struct MetadataPackage {
    id: String,
    #[serde(default)]
    default_run: Option<String>,
}

/// Replaces the running application with `executable`: SIGTERM to the old one's process group,
/// SIGKILL after [`STOP_GRACE`], then the new one on the same held socket.
async fn start(plan: &Plan, running: &mut Option<Running>, executable: PathBuf) -> Result<()> {
    if let Some(current) = running.as_ref().filter(|current| current.executable == executable) {
        eprintln!("{}", "[ulo dev] restarting".dimmed());
        // Stops the old process if it is still running, then starts the new one; a binary that
        // keeps its path keeps its job.
        let _ = current.job.restart_with_signal(Signal::Terminate, STOP_GRACE);
        return Ok(());
    }
    if let Some(old) = running.take() {
        stop(old).await;
    }
    eprintln!("{}", format!("[ulo dev] starting {}", executable.display()).dimmed());
    let command = Arc::new(child_command(plan, &executable)?);
    let (job, task) = start_job(command);
    install_spawn_hook(&job, plan.held.clone());
    let _ = job.set_error_handler(|error| match error.get() {
        Some(error) => eprintln!("{}", format!("[ulo dev] cannot start the application: {error}").red()),
        None => eprintln!("{}", "[ulo dev] cannot start the application".red()),
    });
    let _ = job.start();
    *running = Some(Running { job, task, executable });
    Ok(())
}

async fn stop(running: Running) {
    running.job.stop_with_signal(Signal::Terminate, STOP_GRACE).await;
    running.job.delete_now().await;
    let _ = running.task.await;
}

/// The child: `executable` itself, or, when a socket is held, `ulo __exec -- executable`, which
/// sets `LISTEN_PID` after the exec where setting it is safe.
fn child_command(plan: &Plan, executable: &Path) -> Result<Command> {
    let program = match &plan.trampoline {
        Some(ulo) => {
            let executable = executable
                .to_str()
                .ok_or_else(|| anyhow!("the binary path {} is not UTF-8", executable.display()))?;
            let mut args = vec!["__exec".to_string(), "--".to_string(), executable.to_string()];
            args.extend(plan.app_args.iter().cloned());
            Program::Exec { prog: ulo.clone(), args }
        }
        None => Program::Exec { prog: executable.to_path_buf(), args: plan.app_args.clone() },
    };
    // Grouped, so the stop reaches whatever the application spawned, and so the terminal's Ctrl-C
    // reaches this command alone, which then stops the application in order.
    Ok(Command { program, options: SpawnOptions { grouped: true, ..Default::default() } })
}

/// Sets each child's environment and, with a held socket, places it at descriptor 3. Installed
/// once per job; the job calls it before every spawn.
fn install_spawn_hook(job: &Job, held: Option<Arc<Held>>) {
    let _ = job.set_spawn_hook(move |command, _| {
        let command = command.command_mut();
        // The trampoline sets `LISTEN_PID`; one inherited from this command's own environment
        // names this process, not the child.
        command.env(DEV_VAR, "1").env_remove("LISTEN_PID");
        match &held {
            Some(held) => {
                command.env("LISTEN_FDS", "1").env("LISTEN_FDNAMES", &held.name);
                #[cfg(unix)]
                {
                    use std::os::fd::AsRawFd;

                    // The closure keeps the listener owned: the child inherits a duplicate, and
                    // the original outlives every restart.
                    let source_fd = held.listener.as_raw_fd();
                    // SAFETY: pre_exec runs its closure in exactly the window place_listen_fd
                    // requires.
                    unsafe {
                        command.pre_exec(move || place_listen_fd(source_fd));
                    }
                }
            }
            None => {
                command.env_remove("LISTEN_FDS").env_remove("LISTEN_FDNAMES");
            }
        }
    });
}
