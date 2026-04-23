//! Application health checks with toni-terminus
//!
//! Two endpoints following the Kubernetes liveness/readiness probe pattern:
//!
//!   GET /health/live  — is the process running? (lightweight, no external calls)
//!   GET /health/ready — can it serve traffic? (checks disk, memory, and an HTTP dependency)
//!
//! Both return JSON with status "ok" / "error" and HTTP 200 / 503.
//!
//! Run with:
//!   cargo run --example health_checks
//!
//! Test:
//!   curl -s http://127.0.0.1:3000/health/live  | jq
//!   curl -s http://127.0.0.1:3000/health/ready | jq

use toni::*;
use toni_axum::AxumAdapter;
use toni_terminus::{
    DiskHealthIndicator, HealthCheckService, HttpHealthIndicator, MemoryHealthIndicator,
    TerminusModule,
};

#[controller("/health", pub struct HealthController {
    #[inject] health: HealthCheckService,
    #[inject] http:   HttpHealthIndicator,
    #[inject] memory: MemoryHealthIndicator,
    #[inject] disk:   DiskHealthIndicator,
})]
impl HealthController {
    /// Liveness probe — is the process alive?
    ///
    /// Kubernetes restarts the pod when this returns 503. Keep it cheap:
    /// no network calls, no DB queries.
    #[get("/live")]
    async fn liveness(&self) -> impl IntoResponse {
        self.health
            .check(vec![
                self.memory.check_rss("memory_rss", 512 * 1024 * 1024),
            ])
            .await
    }

    /// Readiness probe — can the process serve traffic?
    ///
    /// Kubernetes stops routing requests when this returns 503, without
    /// restarting the pod. Checks external dependencies here.
    #[get("/ready")]
    async fn readiness(&self) -> impl IntoResponse {
        self.health
            .check(vec![
                self.http
                    .ping_check("httpbin", "https://httpbin.org/get"),
                self.memory.check_rss("memory_rss", 512 * 1024 * 1024),
                self.disk.check_storage("disk", "/", 5.0),
            ])
            .await
    }
}

#[module(
    imports: [TerminusModule::new()],
    controllers: [HealthController],
)]
impl AppModule {}

#[tokio::main]
async fn main() {
    println!("Health checks example\n");
    println!("  GET http://127.0.0.1:3000/health/live  — liveness probe");
    println!("  GET http://127.0.0.1:3000/health/ready — readiness probe");
    println!();

    let mut app = ToniFactory::new()
        .create_with(AppModule::module_definition())
        .await;

    app.use_http_adapter(AxumAdapter::new(), 3000, "127.0.0.1")
        .unwrap();

    app.start().await;
}
