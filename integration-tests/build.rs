fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Vendor protoc so the test suite doesn't depend on a system install.
    let protoc = protoc_bin_vendored::protoc_bin_path()?;
    // SAFETY: build.rs is single-threaded; setting an env var here is fine.
    unsafe {
        std::env::set_var("PROTOC", protoc);
    }
    // The descriptor set is what a reflection service serves: the compiled
    // schema, so a client can discover the API without holding the `.proto`.
    let descriptor =
        std::path::PathBuf::from(std::env::var("OUT_DIR")?).join("orders_descriptor.bin");
    tonic_prost_build::configure()
        .file_descriptor_set_path(&descriptor)
        .compile_protos(&["proto/orders.proto"], &["proto"])?;

    // A service whose Rust method name and route name diverge, which the proto
    // path cannot produce: prost derives one from the other. `grpc_manual_trait_form`
    // serves it, and it is the shape `#[grpc_stream(...)]` exists for.
    let watcher = tonic_build::manual::Service::builder()
        .name("Watcher")
        .package("toni_test.watch")
        .method(
            tonic_build::manual::Method::builder()
                .name("watch")
                .route_name("StreamProgress")
                .input_type("crate::grpc_manual_trait_form::msgs::WatchRequest")
                .output_type("crate::grpc_manual_trait_form::msgs::ProgressEvent")
                .codec_path("tonic_prost::ProstCodec")
                .server_streaming()
                .build(),
        )
        .build();
    tonic_build::manual::Builder::new().compile(&[watcher]);

    Ok(())
}
