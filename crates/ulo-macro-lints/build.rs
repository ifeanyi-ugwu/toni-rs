fn main() -> Result<(), Box<dyn std::error::Error>> {
    ulo_build::configure().compile(&["proto/lints.proto"], &["proto"])?;
    println!("cargo::rerun-if-changed=proto/lints.proto");
    Ok(())
}
