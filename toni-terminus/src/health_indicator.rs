use futures::future::BoxFuture;

use crate::health_check_result::HealthIndicatorResult;

/// A custom health indicator.
///
/// Implement this trait when the built-in indicators don't cover your use case.
/// The implementor is responsible for cloning any state it needs before boxing
/// the future so that the returned future is `'static`.
///
/// # Example
///
/// ```ignore
/// pub struct MyServiceIndicator {
///     client: reqwest::Client,
/// }
///
/// impl HealthIndicator for MyServiceIndicator {
///     fn check(&self, key: &str) -> BoxFuture<'static, HealthIndicatorResult> {
///         let key = key.to_string();
///         let client = self.client.clone();
///         Box::pin(async move {
///             match client.get("https://my-service/ping").send().await {
///                 Ok(_) => Ok(HealthEntry::up(key)),
///                 Err(e) => Err(HealthEntry::down_with(key, json!({ "message": e.to_string() }))),
///             }
///         })
///     }
/// }
/// ```
///
/// Pass the future to [`HealthCheckService::check`]:
///
/// ```ignore
/// self.health.check(vec![
///     self.my_service.check("my-service"),
/// ]).await
/// ```
pub trait HealthIndicator: Send + Sync {
    fn check(&self, key: &str) -> BoxFuture<'static, HealthIndicatorResult>;
}
