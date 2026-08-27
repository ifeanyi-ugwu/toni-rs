use std::{any::Any, future::Future, sync::Arc, time::Instant};

use async_trait::async_trait;
use futures::future::BoxFuture;
use reqwest::{Client, Response};
use serde_json::json;
use toni::{
    FxHashMap,
    traits_helpers::{Injectable, Provider, ProviderContext, ProviderFactory},
};

use crate::health_check_result::{HealthEntry, HealthIndicatorResult};

/// Checks external URLs by making an HTTP GET request and verifying the response
/// is successful (2xx) or a redirect (3xx).
///
/// # Example
///
/// ```ignore
/// self.health.check(vec![
///     self.http.ping_check("docs", "https://docs.example.com"),
///     self.http.ping_check("api",  "https://api.example.com/health"),
/// ]).await
/// ```
#[derive(Clone)]
pub struct HttpHealthIndicator {
    client: Client,
}

impl HttpHealthIndicator {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }

    /// Ping `url` and return a `'static` future that resolves to the check result.
    ///
    /// Passes when the response status is 2xx or 3xx. The response time (ms) is
    /// included in the details on both success and failure.
    pub fn ping_check(
        &self,
        key: impl Into<String>,
        url: impl Into<String>,
    ) -> BoxFuture<'static, HealthIndicatorResult> {
        let client = self.client.clone();
        let key = key.into();
        let url = url.into();

        Box::pin(async move {
            let start = Instant::now();

            match client.get(&url).send().await {
                Ok(resp) if resp.status().is_success() || resp.status().is_redirection() => {
                    let ms = start.elapsed().as_millis();
                    Ok(HealthEntry::up_with(
                        key,
                        json!({ "url": url, "responseTime": ms }),
                    ))
                }
                Ok(resp) => {
                    let ms = start.elapsed().as_millis();
                    Err(HealthEntry::down_with(
                        key,
                        json!({
                            "url": url,
                            "statusCode": resp.status().as_u16(),
                            "responseTime": ms,
                        }),
                    ))
                }
                Err(e) => Err(HealthEntry::down_with(
                    key,
                    json!({ "url": url, "message": e.to_string() }),
                )),
            }
        })
    }

    /// Fetch `url` and pass the response to `validator`. Passes when the validator
    /// returns `true`, fails otherwise.
    ///
    /// Unlike [`ping_check`], this gives you full control — inspect the status
    /// code, headers, or body before deciding:
    ///
    /// ```ignore
    /// // Fail unless the JSON body contains `"ready": true`
    /// self.http.response_check("api", "https://api.example.com/status", |resp| {
    ///     Box::pin(async move {
    ///         resp.json::<serde_json::Value>().await
    ///             .map(|v| v["ready"] == true)
    ///             .unwrap_or(false)
    ///     })
    /// })
    /// ```
    ///
    /// [`ping_check`]: HttpHealthIndicator::ping_check
    pub fn response_check<F, Fut>(
        &self,
        key: impl Into<String>,
        url: impl Into<String>,
        validator: F,
    ) -> BoxFuture<'static, HealthIndicatorResult>
    where
        F: FnOnce(Response) -> Fut + Send + 'static,
        Fut: Future<Output = bool> + Send,
    {
        let client = self.client.clone();
        let key = key.into();
        let url = url.into();

        Box::pin(async move {
            let start = Instant::now();

            match client.get(&url).send().await {
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    let ms = start.elapsed().as_millis();
                    let is_healthy = validator(resp).await;

                    if is_healthy {
                        Ok(HealthEntry::up_with(
                            key,
                            json!({ "url": url, "statusCode": status, "responseTime": ms }),
                        ))
                    } else {
                        Err(HealthEntry::down_with(
                            key,
                            json!({ "url": url, "statusCode": status, "responseTime": ms }),
                        ))
                    }
                }
                Err(e) => Err(HealthEntry::down_with(
                    key,
                    json!({ "url": url, "message": e.to_string() }),
                )),
            }
        })
    }
}

impl Default for HttpHealthIndicator {
    fn default() -> Self {
        Self::new()
    }
}

// ── DI machinery ─────────────────────────────────────────────────────────────

pub(crate) struct HttpHealthIndicatorFactory;

#[async_trait]
impl ProviderFactory for HttpHealthIndicatorFactory {
    fn get_token(&self) -> String {
        toni::di::token_of::<HttpHealthIndicator>()
    }

    async fn build(&self, _deps: FxHashMap<String, Injectable>) -> Injectable {
        let provider = HttpHealthIndicatorProvider {
            indicator: HttpHealthIndicator::new(),
        };
        Injectable::new(Arc::new(Box::new(provider)), vec![])
    }
}

struct HttpHealthIndicatorProvider {
    indicator: HttpHealthIndicator,
}

#[async_trait]
impl Provider for HttpHealthIndicatorProvider {
    fn get_token(&self) -> String {
        toni::di::token_of::<HttpHealthIndicator>()
    }

    async fn execute(
        &self,
        _params: Vec<Box<dyn Any + Send>>,
        _ctx: ProviderContext,
    ) -> Box<dyn Any + Send> {
        Box::new(self.indicator.clone())
    }
}
