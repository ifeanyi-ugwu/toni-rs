use std::path::PathBuf;
use std::process::Command;

/// The compiler `tests/ui/*.stderr` were written by.
const SNAPSHOT_RUSTC: &str = "1.98.1";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").ok_or("cargo sets OUT_DIR for a build script")?);
    ulo_build::configure()
        .file_descriptor_set_path(out_dir.join("codegen.bin"))
        .compile(&["proto/counter.proto"], &["proto"])?;
    println!("cargo::rerun-if-changed=proto/counter.proto");

    println!("cargo::rustc-check-cfg=cfg(ulo_snapshot_rustc)");
    if rustc_release().as_deref() == Some(SNAPSHOT_RUSTC) {
        println!("cargo::rustc-cfg=ulo_snapshot_rustc");
    }
    Ok(())
}

/// The `release:` line of `$RUSTC -vV`, `None` when the compiler cannot be asked.
fn rustc_release() -> Option<String> {
    let rustc = std::env::var_os("RUSTC")?;
    let output = Command::new(rustc).arg("-vV").output().ok()?;
    let text = String::from_utf8(output.stdout).ok()?;
    text.lines().find_map(|line| line.strip_prefix("release: ")).map(str::to_owned)
}
