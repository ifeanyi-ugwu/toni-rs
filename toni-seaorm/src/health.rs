use std::{any::Any, sync::Arc};

use async_trait::async_trait;
use futures::future::BoxFuture;
use sea_orm::{Database, DatabaseConnection};
use toni::{
    FxHashMap,
    traits_helpers::{Injectable, Provider, ProviderContext, ProviderFactory},
};
use toni_terminus::{HealthEntry, HealthIndicator, HealthIndicatorResult};

#[derive(Clone)]
pub struct SeaOrmHealthIndicator {
    db: DatabaseConnection,
}

impl SeaOrmHealthIndicator {
    pub fn ping_check(&self, key: &str) -> BoxFuture<'static, HealthIndicatorResult> {
        let key = key.to_string();
        let db = self.db.clone();
        Box::pin(async move {
            match db.ping().await {
                Ok(_) => Ok(HealthEntry::up(key)),
                Err(e) => Err(HealthEntry::down_with(
                    key,
                    serde_json::json!({ "message": e.to_string() }),
                )),
            }
        })
    }
}

impl HealthIndicator for SeaOrmHealthIndicator {
    fn check(&self, key: &str) -> BoxFuture<'static, HealthIndicatorResult> {
        self.ping_check(key)
    }
}

// ── DI machinery ─────────────────────────────────────────────────────────────

pub(crate) struct SeaOrmHealthIndicatorFactory {
    pub database_url: String,
}

#[async_trait]
impl ProviderFactory for SeaOrmHealthIndicatorFactory {
    fn get_token(&self) -> String {
        std::any::type_name::<SeaOrmHealthIndicator>().to_string()
    }

    async fn build(&self, _deps: FxHashMap<String, Injectable>) -> Injectable {
        let db = Database::connect(&self.database_url)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "toni-seaorm health: failed to connect to '{}': {e}",
                    self.database_url
                )
            });
        Injectable::new(
            Arc::new(Box::new(SeaOrmHealthProvider {
                indicator: SeaOrmHealthIndicator { db },
            })),
            vec![],
        )
    }
}

struct SeaOrmHealthProvider {
    indicator: SeaOrmHealthIndicator,
}

#[async_trait]
impl Provider for SeaOrmHealthProvider {
    fn get_token(&self) -> String {
        std::any::type_name::<SeaOrmHealthIndicator>().to_string()
    }

    fn get_token_factory(&self) -> String {
        std::any::type_name::<SeaOrmHealthIndicator>().to_string()
    }

    async fn execute(
        &self,
        _params: Vec<Box<dyn Any + Send>>,
        _ctx: ProviderContext,
    ) -> Box<dyn Any + Send> {
        Box::new(self.indicator.clone())
    }
}
