use std::{any::Any, sync::Arc};

use async_trait::async_trait;
use futures::future::BoxFuture;
use toni::{
    FxHashMap,
    traits_helpers::{Injectable, Provider, ProviderContext, ProviderFactory},
};

use crate::health_check_result::{HealthCheckResult, HealthIndicatorResult};

/// Runs a set of health checks and aggregates the results.
///
/// Inject this into your health controller, then call [`check`] with a vec
/// of futures produced by your indicators:
///
/// ```ignore
/// #[get("/")]
/// async fn liveness(&self) -> impl IntoResponse {
///     self.health.check(vec![
///         self.http.ping_check("api", "https://api.example.com"),
///         self.memory.check_heap("memory", 300 * 1024 * 1024),
///     ]).await
/// }
/// ```
///
/// Returns HTTP 200 when all checks pass, HTTP 503 when any fail.
#[derive(Clone)]
pub struct HealthCheckService;

impl HealthCheckService {
    pub async fn check(
        &self,
        checks: Vec<BoxFuture<'static, HealthIndicatorResult>>,
    ) -> HealthCheckResult {
        let results = futures::future::join_all(checks).await;
        HealthCheckResult::from_results(results)
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
