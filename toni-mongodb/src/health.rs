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
        let (indicator, init_error) = match ClientOptions::parse(&self.uri).await {
            Err(e) => (
                None,
                Some(crate::redact::describe("failed to parse URI", e, &self.uri)),
            ),
            Ok(options) => match Client::with_options(options) {
                Ok(client) => (
                    Some(MongoHealthIndicator {
                        db: client.database(&self.db_name),
                    }),
                    None,
                ),
                Err(e) => (
                    None,
                    Some(crate::redact::describe(
                        "failed to create client",
                        e,
                        &self.uri,
                    )),
                ),
            },
        };
        Injectable::new(
            Arc::new(Box::new(MongoHealthProvider {
                indicator,
                init_error,
            })),
            vec![],
        )
    }
}

struct MongoHealthProvider {
    indicator: Option<MongoHealthIndicator>,
    // Set when the client could not be constructed; reported from `on_module_init`.
    init_error: Option<String>,
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
        Box::new(
            self.indicator
                .clone()
                .expect("health indicator unavailable"),
        )
    }
    async fn on_module_init(&self) -> toni::InitResult {
        match &self.init_error {
            Some(message) => Err(message.clone().into()),
            None => Ok(()),
        }
    }
}
