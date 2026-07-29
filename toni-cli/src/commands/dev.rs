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

#[derive(clap::Args)]
pub struct DevArgs {
    /// Build and run in release mode
    #[arg(long)]
    release: bool,

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

    let job_id = watchexec::Id::default();
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
        let job = action.get_or_create_job(job_id, || command.clone());
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
