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

pub(crate) struct MongoHealthIndicatorFactory {
    pub uri: String,
    pub db_name: String,
}

#[async_trait]
impl ProviderFactory for MongoHealthIndicatorFactory {
    fn get_token(&self) -> String {
        std::any::type_name::<MongoHealthIndicator>().to_string()
    }

    async fn build(&self, _deps: FxHashMap<String, Injectable>) -> Injectable {
        let options = ClientOptions::parse(&self.uri).await.unwrap_or_else(|e| {
            panic!(
                "toni-mongodb health: failed to parse URI '{}': {e}",
                self.uri
            )
        });
        let client = Client::with_options(options)
            .unwrap_or_else(|e| panic!("toni-mongodb health: failed to create client: {e}"));
        let db = client.database(&self.db_name);
        Injectable::new(
            Arc::new(Box::new(MongoHealthProvider {
                indicator: MongoHealthIndicator { db },
            })),
            vec![],
        )
    }
}

struct MongoHealthProvider {
    indicator: MongoHealthIndicator,
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
        _ctx: ProviderContext<'_>,
    ) -> Box<dyn Any + Send> {
        Box::new(self.indicator.clone())
    }
}
