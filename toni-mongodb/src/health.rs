use std::{any::Any, sync::Arc};

use async_trait::async_trait;
use futures::future::BoxFuture;
use mongodb::{Client, Database, options::ClientOptions};
use toni::{
    FxHashMap,
    traits_helpers::{Injectable, Provider, ProviderContext, ProviderFactory},
};
use toni_terminus::{HealthEntry, HealthIndicator, HealthIndicatorResult};

#[derive(Clone)]
pub struct MongoHealthIndicator {
    db: Database,
}

impl MongoHealthIndicator {
    pub fn ping_check(&self, key: &str) -> BoxFuture<'static, HealthIndicatorResult> {
        let key = key.to_string();
        let db = self.db.clone();
        Box::pin(async move {
            match db.run_command(mongodb::bson::doc! { "ping": 1 }).await {
                Ok(_) => Ok(HealthEntry::up(key)),
                Err(e) => Err(HealthEntry::down_with(
                    key,
                    serde_json::json!({ "message": e.to_string() }),
                )),
            }
        })
    }
}

impl HealthIndicator for MongoHealthIndicator {
    fn check(&self, key: &str) -> BoxFuture<'static, HealthIndicatorResult> {
        self.ping_check(key)
    }
}

// ── DI machinery ─────────────────────────────────────────────────────────────

pub(crate) struct MongoHealthIndicatorFactory;

#[async_trait]
impl ProviderFactory for MongoHealthIndicatorFactory {
    fn get_token(&self) -> String {
        std::any::type_name::<MongoHealthIndicator>().to_string()
    }

    fn get_dependencies(&self) -> Vec<String> {
        vec![std::any::type_name::<Database>().to_string()]
    }

    async fn build(&self, deps: FxHashMap<String, Injectable>) -> Injectable {
        let token = std::any::type_name::<Database>().to_string();
        let connection = deps
            .get(&token)
            .expect("the health indicator is registered alongside the connection it checks")
            .instance
            .clone();
        Injectable::new(
            Arc::new(Box::new(MongoHealthProvider { connection })),
            vec![],
        )
    }
}

struct MongoHealthProvider {
    // The registered connection's provider, resolved per request for an indicator rather than at
    // build time: the connection may have failed, and startup reports that from its own
    // `on_module_init` before anything can resolve this one.
    connection: Arc<Box<dyn Provider>>,
}

#[async_trait]
impl Provider for MongoHealthProvider {
    fn get_token(&self) -> String {
        std::any::type_name::<MongoHealthIndicator>().to_string()
    }

    fn get_token_factory(&self) -> String {
        std::any::type_name::<MongoHealthIndicator>().to_string()
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
        let db = *resolved
            .downcast::<Database>()
            .expect("the registered connection provider yields a Database");
        Box::new(MongoHealthIndicator { db })
    }
}
