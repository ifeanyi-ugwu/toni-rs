//! Startup failures written to files: a scenario whose environment did not start fails through
//! [`startup_failed!`](crate::startup_failed), which writes the message, with its full error chain,
//! to a file of its own before panicking with it, so the cause survives a runner that keeps only
//! part of the test output. CI uploads the directory when a job fails.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Where startup failures are written: `$ULO_CONFORMANCE_FAILURES` when it is set, otherwise
/// `target/conformance-failures` at the workspace root, whatever `CARGO_TARGET_DIR` says.
pub fn failures_dir() -> PathBuf {
    match std::env::var_os("ULO_CONFORMANCE_FAILURES") {
        Some(dir) => PathBuf::from(dir),
        None => {
            let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
            manifest.ancestors().nth(2).unwrap_or(manifest).join("target").join("conformance-failures")
        }
    }
}

/// Writes `message` under [`failures_dir`], then panics with it and the file's path, at the
/// caller's location.
#[track_caller]
pub fn startup_failed(message: String) -> ! {
    match write(&message) {
        Ok(path) => panic!("{message}\n(written to {})", path.display()),
        Err(error) => panic!("{message}\n(not written under {}: {error})", failures_dir().display()),
    }
}

/// One file per failure, named after the package under test, the test (libtest names the thread
/// running it after the test), the process and the time.
fn write(message: &str) -> io::Result<PathBuf> {
    let dir = failures_dir();
    fs::create_dir_all(&dir)?;
    let binary = std::env::current_exe().ok();
    let suite = std::env::var("CARGO_PKG_NAME").ok().or_else(|| {
        binary.as_ref().and_then(|exe| exe.file_stem()).map(|stem| stem.to_string_lossy().into_owned())
    });
    let suite = suite.unwrap_or_else(|| "unknown".to_owned());
    let thread = std::thread::current();
    let test = thread.name().unwrap_or("unnamed");
    let since = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let name: String = format!("{suite}-{test}-{}-{}.txt", std::process::id(), since.as_nanos())
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '_' })
        .collect();
    let path = dir.join(name);
    let binary = binary.map_or_else(String::new, |exe| exe.display().to_string());
    fs::write(&path, format!("package: {suite}\ntest: {test}\nbinary: {binary}\nunix time: {}\n\n{message}\n", since.as_secs()))?;
    Ok(path)
}
