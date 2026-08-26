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

pub(crate) struct RedisHealthIndicatorFactory;

#[async_trait]
impl ProviderFactory for RedisHealthIndicatorFactory {
    fn get_token(&self) -> String {
        toni::di::token_of::<RedisHealthIndicator>()
    }

    fn get_dependencies(&self) -> Vec<String> {
        vec![toni::di::token_of::<ConnectionManager>()]
    }

    async fn build(&self, deps: FxHashMap<String, Injectable>) -> Injectable {
        let token = toni::di::token_of::<ConnectionManager>();
        let connection = deps
            .get(&token)
            .expect("the health indicator is registered alongside the connection it checks")
            .instance
            .clone();
        Injectable::new(
            Arc::new(Box::new(RedisHealthProvider { connection })),
            vec![],
        )
    }
}

struct RedisHealthProvider {
    // The registered connection's provider, resolved per request for an indicator rather than at
    // build time: the connection may have failed, and startup reports that from its own
    // `on_module_init` before anything can resolve this one.
    connection: Arc<Box<dyn Provider>>,
}

#[async_trait]
impl Provider for RedisHealthProvider {
    fn get_token(&self) -> String {
        toni::di::token_of::<RedisHealthIndicator>()
    }

    fn get_token_factory(&self) -> String {
        toni::di::token_of::<RedisHealthIndicator>()
    }

    async fn execute(
        &self,
        _params: Vec<Box<dyn Any + Send>>,
        _ctx: ProviderContext,
    ) -> Box<dyn Any + Send> {
        let resolved = self
            .connection
            .execute(Vec::new(), ProviderContext::None)
            .await;
        let manager = *resolved
            .downcast::<ConnectionManager>()
            .expect("the registered connection provider yields a ConnectionManager");
        Box::new(RedisHealthIndicator { manager })
    }
}
