use toni::DynamicModule;

use crate::health_check_service::HealthCheckServiceFactory;

#[cfg(feature = "disk")]
use crate::indicators::disk::DiskHealthIndicatorFactory;
#[cfg(feature = "http")]
use crate::indicators::http::HttpHealthIndicatorFactory;
#[cfg(feature = "memory")]
use crate::indicators::memory::MemoryHealthIndicatorFactory;

#[cfg(feature = "disk")]
use crate::indicators::DiskHealthIndicator;
#[cfg(feature = "http")]
use crate::indicators::HttpHealthIndicator;
#[cfg(feature = "memory")]
use crate::indicators::MemoryHealthIndicator;

use crate::health_check_service::HealthCheckService;

pub struct TerminusModule;

impl TerminusModule {
    /// Register the health check infrastructure.
    ///
    /// Always provides [`HealthCheckService`]. Each feature flag adds its
    /// indicator automatically:
    ///
    /// | Feature  | Indicator added               |
    /// |----------|-------------------------------|
    /// | `http`   | [`HttpHealthIndicator`]       |
    /// | `memory` | [`MemoryHealthIndicator`]     |
    /// | `disk`   | [`DiskHealthIndicator`]       |
    ///
    /// Import once in your root module:
    ///
    /// ```ignore
    /// #[module(imports: [TerminusModule::new(), /* other modules */])]
    /// pub struct AppModule;
    /// ```
    ///
    /// Then inject any indicator you need:
    ///
    /// ```ignore
    /// #[controller("/health")]
    /// pub struct HealthController {
    ///     #[inject] health: HealthCheckService,
    ///     #[inject] http:   HttpHealthIndicator,
    /// }
    ///
    /// #[routes]
    /// impl HealthController {
    ///     #[get("/live")]
    ///     async fn liveness(&self) -> impl IntoResponse {
    ///         self.health.check(vec![
    ///             self.http.ping_check("api", "https://api.example.com"),
    ///         ]).await
    ///     }
    /// }
    /// ```
    pub fn new() -> DynamicModule {
        #[allow(unused_mut)]
        let mut builder = DynamicModule::builder("TerminusModule")
            .provider(HealthCheckServiceFactory)
            .export::<HealthCheckService>();

        #[cfg(feature = "http")]
        {
            builder = builder
                .provider(HttpHealthIndicatorFactory)
                .export::<HttpHealthIndicator>();
        }

        #[cfg(feature = "memory")]
        {
            builder = builder
                .provider(MemoryHealthIndicatorFactory)
                .export::<MemoryHealthIndicator>();
        }

        #[cfg(feature = "disk")]
        {
            builder = builder
                .provider(DiskHealthIndicatorFactory)
                .export::<DiskHealthIndicator>();
        }

        builder.global().build()
    }
}
