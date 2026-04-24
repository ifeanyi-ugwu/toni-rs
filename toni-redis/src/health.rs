use std::{any::Any, sync::Arc};

use async_trait::async_trait;
use futures::future::BoxFuture;
use redis::aio::ConnectionManager;
use toni::{
    FxHashMap,
    traits_helpers::{Injectable, Provider, ProviderContext, ProviderFactory},
};
use toni_terminus::{HealthEntry, HealthIndicator, HealthIndicatorResult};

#[derive(Clone)]
pub struct RedisHealthIndicator {
    manager: ConnectionManager,
}

impl RedisHealthIndicator {
    pub fn ping_check(&self, key: &str) -> BoxFuture<'static, HealthIndicatorResult> {
        let key = key.to_string();
        let mut manager = self.manager.clone();
        Box::pin(async move {
            match redis::cmd("PING").query_async::<String>(&mut manager).await {
                Ok(_) => Ok(HealthEntry::up(key)),
                Err(e) => Err(HealthEntry::down_with(
                    key,
                    serde_json::json!({ "message": e.to_string() }),
                )),
            }
        })
    }
}

impl HealthIndicator for RedisHealthIndicator {
    fn check(&self, key: &str) -> BoxFuture<'static, HealthIndicatorResult> {
        self.ping_check(key)
    }
}

// ── DI machinery ─────────────────────────────────────────────────────────────

pub(crate) struct RedisHealthIndicatorFactory {
    pub url: String,
}

#[async_trait]
impl ProviderFactory for RedisHealthIndicatorFactory {
    fn get_token(&self) -> String {
        std::any::type_name::<RedisHealthIndicator>().to_string()
    }

    async fn build(&self, _deps: FxHashMap<String, Injectable>) -> Injectable {
        let client = redis::Client::open(self.url.as_str())
            .unwrap_or_else(|e| panic!("toni-redis health: invalid URL '{}': {e}", self.url));
        let manager = ConnectionManager::new(client)
            .await
            .unwrap_or_else(|e| {
                panic!("toni-redis health: failed to connect to '{}': {e}", self.url)
            });
        Injectable::new(
            Arc::new(Box::new(RedisHealthProvider {
                indicator: RedisHealthIndicator { manager },
            })),
            vec![],
        )
    }
}

struct RedisHealthProvider {
    indicator: RedisHealthIndicator,
}

#[async_trait]
impl Provider for RedisHealthProvider {
    fn get_token(&self) -> String {
        std::any::type_name::<RedisHealthIndicator>().to_string()
    }

    fn get_token_factory(&self) -> String {
        std::any::type_name::<RedisHealthIndicator>().to_string()
    }

    async fn execute(
        &self,
        _params: Vec<Box<dyn Any + Send>>,
        _ctx: ProviderContext<'_>,
    ) -> Box<dyn Any + Send> {
        Box::new(self.indicator.clone())
    }
}
