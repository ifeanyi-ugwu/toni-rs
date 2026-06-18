use std::{any::Any, future::IntoFuture, sync::Arc};

use async_trait::async_trait;
use futures::future::{BoxFuture, join_all};
use toni::{
    FxHashMap,
    traits_helpers::{Injectable, Provider, ProviderContext, ProviderFactory},
};

use crate::health_check_result::{HealthCheckResult, HealthIndicatorResult};

#[cfg(feature = "timeout")]
use crate::health_check_result::HealthEntry;

/// Runs a set of health checks and aggregates the results.
///
/// Inject this into your health controller, then call [`check`] with a vec
/// of futures produced by your indicators:
///
/// ```ignore
/// #[get("/live")]
/// async fn liveness(&self) -> impl IntoResponse {
///     self.health.check(vec![
///         self.http.ping_check("api", "https://api.example.com"),
///         self.memory.check_rss("memory", 300 * 1024 * 1024),
///     ]).await
/// }
/// ```
///
/// Chain `.timeout(Duration)` before `.await` to bound each check:
///
/// ```ignore
/// self.health
///     .check(vec![self.http.ping_check("api", "https://api.example.com")])
///     .timeout(Duration::from_secs(3))
///     .await
/// ```
///
/// Returns HTTP 200 when all checks pass, HTTP 503 when any fail.
///
/// [`check`]: HealthCheckService::check
#[derive(Clone)]
pub struct HealthCheckService;

impl HealthCheckService {
    pub fn check(
        &self,
        checks: Vec<BoxFuture<'static, HealthIndicatorResult>>,
    ) -> HealthCheckBuilder {
        HealthCheckBuilder::new(checks)
    }
}

// ── Builder ───────────────────────────────────────────────────────────────────

/// Returned by [`HealthCheckService::check`]. Awaitable directly, or chain
/// `.timeout(Duration)` first (requires the `timeout` feature).
pub struct HealthCheckBuilder {
    checks: Vec<BoxFuture<'static, HealthIndicatorResult>>,
    #[cfg(feature = "timeout")]
    timeout: Option<std::time::Duration>,
}

impl HealthCheckBuilder {
    pub(crate) fn new(checks: Vec<BoxFuture<'static, HealthIndicatorResult>>) -> Self {
        Self {
            checks,
            #[cfg(feature = "timeout")]
            timeout: None,
        }
    }

    /// Set a per-check timeout. Any individual check that does not resolve
    /// within `duration` is marked unhealthy with key `"timed_out"`.
    ///
    /// All checks still run concurrently — the timeout applies to each one
    /// independently, not to the whole group.
    #[cfg(feature = "timeout")]
    pub fn timeout(mut self, duration: std::time::Duration) -> Self {
        self.timeout = Some(duration);
        self
    }
}

impl IntoFuture for HealthCheckBuilder {
    type Output = HealthCheckResult;
    type IntoFuture = BoxFuture<'static, HealthCheckResult>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(run(self))
    }
}

async fn run(builder: HealthCheckBuilder) -> HealthCheckResult {
    #[cfg(feature = "timeout")]
    if let Some(dur) = builder.timeout {
        let wrapped = builder.checks.into_iter().map(|check| {
            Box::pin(async move {
                tokio::time::timeout(dur, check).await.unwrap_or_else(|_| {
                    Err(HealthEntry::down_with(
                        "timed_out",
                        serde_json::json!({
                            "message": "Health check timed out",
                            "timeoutMs": dur.as_millis(),
                        }),
                    ))
                })
            }) as BoxFuture<'static, HealthIndicatorResult>
        });
        let results = join_all(wrapped).await;
        return HealthCheckResult::from_results(results);
    }

    let results = join_all(builder.checks).await;
    HealthCheckResult::from_results(results)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(all(test, feature = "timeout"))]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::health_check_result::HealthEntry;

    #[tokio::test]
    async fn timed_out_check_is_reported_as_error() {
        let health = HealthCheckService;
        let result = health
            .check(vec![Box::pin(async {
                tokio::time::sleep(Duration::from_secs(60)).await;
                Ok(HealthEntry::up("never"))
            })])
            .timeout(Duration::from_millis(50))
            .await;

        assert_eq!(result.status(), "error");
        assert!(!result.is_healthy());
    }

    #[tokio::test]
    async fn fast_check_passes_within_timeout() {
        let health = HealthCheckService;
        let result = health
            .check(vec![Box::pin(async { Ok(HealthEntry::up("fast")) })])
            .timeout(Duration::from_secs(5))
            .await;

        assert_eq!(result.status(), "ok");
    }

    #[tokio::test]
    async fn only_slow_checks_are_marked_timed_out() {
        let health = HealthCheckService;
        let result = health
            .check(vec![
                Box::pin(async { Ok(HealthEntry::up("fast")) }),
                Box::pin(async {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    Ok(HealthEntry::up("slow"))
                }),
            ])
            .timeout(Duration::from_millis(50))
            .await;

        // Overall is error because the slow check timed out.
        assert_eq!(result.status(), "error");
        // But it completed, not hung — the fast check didn't block.
    }
}

// ── DI machinery ─────────────────────────────────────────────────────────────

pub(crate) struct HealthCheckServiceFactory;

#[async_trait]
impl ProviderFactory for HealthCheckServiceFactory {
    fn get_token(&self) -> String {
        std::any::type_name::<HealthCheckService>().to_string()
    }

    async fn build(&self, _deps: FxHashMap<String, Injectable>) -> Injectable {
        Injectable::new(Arc::new(Box::new(HealthCheckServiceProvider)), vec![])
    }
}

struct HealthCheckServiceProvider;

#[async_trait]
impl Provider for HealthCheckServiceProvider {
    fn get_token(&self) -> String {
        std::any::type_name::<HealthCheckService>().to_string()
    }

    fn get_token_factory(&self) -> String {
        std::any::type_name::<HealthCheckService>().to_string()
    }

    async fn execute(
        &self,
        _params: Vec<Box<dyn Any + Send>>,
        _ctx: ProviderContext<'_>,
    ) -> Box<dyn Any + Send> {
        Box::new(HealthCheckService)
    }
}
