//! Seam-level coverage for the gRPC RPC adapter (PR 1):
//!
//! - `app.use_grpc_adapter()` registers a `toni_grpc::GrpcAdapter`
//! - `app.bind().await` surfaces the bound address via `BoundAdapters.grpc`
//! - the adapter actually serves — we make a real gRPC call to a registered
//!   `tonic-health` service and assert it returns `SERVING`
//! - `ShutdownHandle::shutdown` drains the gRPC server cleanly so
//!   `ShutdownHandle::completed().await` resolves
//!
//! Macros, DI integration, streaming, and per-request tracing all land in
//! later PRs. This test only verifies the framework seam works end-to-end.

use std::time::Duration;

use tonic_health::pb::health_check_response::ServingStatus as PbServingStatus;
use tonic_health::pb::health_client::HealthClient;
use tonic_health::pb::HealthCheckRequest;
use tonic_health::ServingStatus;

#[tokio_localset_test::localset_test]
async fn grpc_adapter_seam_round_trip_and_shuts_down() {
    use toni::toni_factory::ToniFactory;

    // Empty module — gRPC services are added directly to the adapter for
    // now (no `#[grpc_service]` macro yet). The framework still requires a
    // module to construct an app.
    #[toni::module()]
    struct EmptyModule;

    // Spin up tonic-health as our smoke-test service — anything tonic-shaped
    // would do, but `tonic_health` doesn't need protobuf codegen.
    let (health_reporter, health_service) = tonic_health::server::health_reporter();
    health_reporter
        .set_service_status("test.service", ServingStatus::Serving)
        .await;

    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let adapter = toni_grpc::GrpcAdapter::new(addr).add_service(health_service);

    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<toni::ShutdownHandle>();

    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = ToniFactory::create(EmptyModule::module_definition()).await;
        app.use_grpc_adapter(adapter).unwrap();
        let bound = app.bind().await.unwrap();
        let port = bound
            .grpc
            .expect("BoundAdapters.grpc must be populated after bind()")
            .port();
        let _ = port_tx.send(port);
        let _ = shutdown_tx.send(app.shutdown_handle());
        app.run().await;
    });
    tokio::task::spawn_local(async move { local.await });

    let port = port_rx.await.unwrap();
    let shutdown = shutdown_rx.await.unwrap();

    // Real gRPC call against the bound address — proves the seam wires
    // the tonic Server through to the listener correctly.
    let endpoint = format!("http://127.0.0.1:{}", port);
    let mut client = tonic::transport::Endpoint::from_shared(endpoint)
        .unwrap()
        .connect()
        .await
        .map(HealthClient::new)
        .expect("gRPC connect should succeed");

    let resp = tokio::time::timeout(
        Duration::from_secs(2),
        client.check(HealthCheckRequest {
            service: "test.service".to_string(),
        }),
    )
    .await
    .expect("health check must reply within 2s")
    .expect("health check must succeed");

    assert_eq!(resp.into_inner().status, PbServingStatus::Serving as i32);

    shutdown.shutdown();
    tokio::time::timeout(Duration::from_secs(2), shutdown.completed())
        .await
        .expect("shutdown must complete within 2s once close() fires");
}
