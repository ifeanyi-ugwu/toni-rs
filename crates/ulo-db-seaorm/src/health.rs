use std::{any::Any, sync::Arc};

use async_trait::async_trait;
use futures::future::BoxFuture;
use sea_orm::DatabaseConnection;
use ulo::{
    FxHashMap,
    di::{Execution, ResolutionError},
    spi::{BuildResult, Injectable, Provider, ProviderFactory},
};
use ulo_health::{HealthEntry, HealthIndicator, HealthIndicatorResult};

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

pub(crate) struct SeaOrmHealthIndicatorFactory;

#[async_trait]
impl ProviderFactory for SeaOrmHealthIndicatorFactory {
    fn token(&self) -> String {
        ulo::di::token_of::<SeaOrmHealthIndicator>()
    }

    fn dependency_tokens(&self) -> Vec<String> {
        vec![ulo::di::token_of::<DatabaseConnection>()]
    }

    async fn build(&self, deps: FxHashMap<String, Injectable>) -> BuildResult<Injectable> {
        let token = ulo::di::token_of::<DatabaseConnection>();
        let connection = deps
            .get(&token)
            .expect("the health indicator is registered alongside the connection it checks")
            .instance
            .clone();
        Ok(Injectable::new(
            Arc::new(Box::new(SeaOrmHealthProvider { connection })),
            vec![],
        ))
    }
}

struct SeaOrmHealthProvider {
    // The registered connection's provider, resolved per request for an indicator rather than at
    // build time: the connection may have failed, and startup reports that from its own
    // `on_module_init` before anything can resolve this one.
    connection: Arc<Box<dyn Provider>>,
}

#[async_trait]
impl Provider for SeaOrmHealthProvider {
    fn token(&self) -> String {
        ulo::di::token_of::<SeaOrmHealthIndicator>()
    }

    async fn resolve(&self, _ctx: Execution) -> Result<Box<dyn Any + Send>, ResolutionError> {
        let resolved = self.connection.resolve(Execution::None).await?;
        let db = *resolved.downcast::<DatabaseConnection>().map_err(|_| {
            ResolutionError::TypeMismatch {
                token: ulo::di::token_of::<DatabaseConnection>(),
            }
        })?;
        Ok(Box::new(SeaOrmHealthIndicator { db }))
    }
}
