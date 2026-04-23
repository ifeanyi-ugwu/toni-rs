//! Application health checks with toni-terminus
//!
//! Demonstrates four patterns:
//!   1. ping_check   — simple 2xx/3xx pass
//!   2. response_check — full response validation via closure
//!   3. Custom HealthIndicator — implement the trait for bespoke checks
//!   4. Timeout — bound slow checks so they don't hang indefinitely
//!
//! Two endpoints follow the Kubernetes liveness/readiness split:
//!
//!   GET /health/live  — lightweight: only memory + uptime (no network)
//!   GET /health/ready — full: external ping, response_check, memory, disk
//!
//! Run with:
//!   cargo run --example health_checks
//!
//! Test:
//!   curl -s http://127.0.0.1:3000/health/live  | jq
//!   curl -s http://127.0.0.1:3000/health/ready | jq

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use serde_json::json;
use toni::*;
use toni_axum::AxumAdapter;
use toni_terminus::{
    DiskHealthIndicator, HealthCheckService, HealthEntry, HealthIndicator, HealthIndicatorResult,
    HttpHealthIndicator, MemoryHealthIndicator, TerminusModule,
};

// ── Custom indicator ──────────────────────────────────────────────────────────

// Track when the process started so the indicator can report uptime.
static APP_START: OnceLock<Instant> = OnceLock::new();

/// Reports how long the process has been running.
///
/// This is the pattern for a custom indicator that needs no injected
/// dependencies — implement [`HealthIndicator`], clone whatever state
/// you need into the returned future so it is `'static`.
struct UptimeIndicator;

impl HealthIndicator for UptimeIndicator {
    fn check(&self, key: &str) -> BoxFuture<'static, HealthIndicatorResult> {
        let key = key.to_string();
        Box::pin(async move {
            let secs = APP_START
                .get()
                .map(|t| t.elapsed().as_secs())
                .unwrap_or(0);
            Ok(HealthEntry::up_with(key, json!({ "uptimeSecs": secs })))
        })
    }
}

// ── Controller ────────────────────────────────────────────────────────────────

#[controller("/health", pub struct HealthController {
    #[inject] health: HealthCheckService,
    #[inject] http:   HttpHealthIndicator,
    #[inject] memory: MemoryHealthIndicator,
    #[inject] disk:   DiskHealthIndicator,
})]
impl HealthController {
    /// Liveness probe — is the process alive?
    ///
    /// No network calls. Kubernetes restarts the pod when this 503s.
    #[get("/live")]
    async fn liveness(&self) -> impl IntoResponse {
        self.health
            .check(vec![
                self.memory.check_rss("memory_rss", 512 * 1024 * 1024),
                UptimeIndicator.check("uptime"),
            ])
            .await
    }

    /// Readiness probe — can the process serve traffic?
    ///
    /// Checks external dependencies. Kubernetes stops routing here (without
    /// restarting) when this 503s.
    ///
    /// Uses a 5-second per-check timeout so a hung external call never blocks
    /// the probe indefinitely (requires the `timeout` feature).
    #[get("/ready")]
    async fn readiness(&self) -> impl IntoResponse {
        self.health
            .check(vec![
                // ping_check: passes on any 2xx or 3xx response
                self.http
                    .ping_check("httpbin", "https://httpbin.org/get"),
                // response_check: full control — inspect status, headers, or body
                self.http.response_check(
                    "httpbin_json",
                    "https://httpbin.org/json",
                    |resp| Box::pin(async move { resp.status().is_success() }),
                ),
                self.memory.check_rss("memory_rss", 512 * 1024 * 1024),
                self.disk.check_storage("disk", "/", 5.0),
            ])
            .timeout(Duration::from_secs(5))
            .await
    }
}

// ── Module & main ─────────────────────────────────────────────────────────────

#[module(
    imports: [TerminusModule::new()],
    controllers: [HealthController],
)]
impl AppModule {}

#[tokio::main]
async fn main() {
    APP_START.get_or_init(Instant::now);

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
