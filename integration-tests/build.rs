fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Vendor protoc so the test suite doesn't depend on a system install.
    let protoc = protoc_bin_vendored::protoc_bin_path()?;
    // SAFETY: build.rs is single-threaded; setting an env var here is fine.
    unsafe {
        std::env::set_var("PROTOC", protoc);
    }
    tonic_prost_build::compile_protos("proto/orders.proto")?;

    // A service whose Rust method name and route name diverge, which the proto
    // path cannot produce: prost derives one from the other. `grpc_stream_optin`
    // serves it, and it is the shape `#[stream(...)]` exists for.
    let watcher = tonic_build::manual::Service::builder()
        .name("Watcher")
        .package("toni_test.watch")
        .method(
            tonic_build::manual::Method::builder()
                .name("watch")
                .route_name("StreamProgress")
                .input_type("crate::grpc_stream_optin::msgs::WatchRequest")
                .output_type("crate::grpc_stream_optin::msgs::ProgressEvent")
                .codec_path("tonic_prost::ProstCodec")
                .server_streaming()
                .build(),
        )
        .build();
    tonic_build::manual::Builder::new().compile(&[watcher]);

    Ok(())
}
