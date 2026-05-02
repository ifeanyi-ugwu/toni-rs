fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Vendor protoc so the test suite doesn't depend on a system install.
    let protoc = protoc_bin_vendored::protoc_bin_path()?;
    // SAFETY: build.rs is single-threaded; setting an env var here is fine.
    unsafe {
        std::env::set_var("PROTOC", protoc);
    }
    tonic_prost_build::compile_protos("proto/orders.proto")?;
    Ok(())
}
