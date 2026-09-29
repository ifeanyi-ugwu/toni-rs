use std::{any::Any, sync::Arc};

use async_trait::async_trait;
use futures::future::BoxFuture;
use mongodb::{Client, Database, options::ClientOptions};
use ulo::{
    FxHashMap,
    di::{Execution, ResolutionError},
    spi::{BuildResult, Provider, ProviderFactory, Registration},
};
use ulo_health::{HealthEntry, HealthIndicator, HealthIndicatorResult};

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
    fn token(&self) -> String {
        ulo::di::token_of::<MongoHealthIndicator>()
    }

    fn dependency_tokens(&self) -> Vec<String> {
        vec![ulo::di::token_of::<Database>()]
    }

    async fn build(&self, deps: FxHashMap<String, Registration>) -> BuildResult<Registration> {
        let token = ulo::di::token_of::<Database>();
        let connection = deps
            .get(&token)
            .expect("the health indicator is registered alongside the connection it checks")
            .instance
            .clone();
        Ok(Registration::new(
            Arc::new(Box::new(MongoHealthProvider { connection })),
            vec![],
        ))
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
    fn token(&self) -> String {
        ulo::di::token_of::<MongoHealthIndicator>()
    }

    async fn resolve(&self, _ctx: Execution) -> Result<Box<dyn Any + Send>, ResolutionError> {
        let resolved = self.connection.resolve(Execution::None).await?;
        let db = *resolved
            .downcast::<Database>()
            .map_err(|_| ResolutionError::TypeMismatch {
                token: ulo::di::token_of::<Database>(),
            })?;
        Ok(Box::new(MongoHealthIndicator { db }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A connection provider answering a value of another type.
    struct Mistyped;

    #[async_trait]
    impl Provider for Mistyped {
        fn token(&self) -> String {
            ulo::di::token_of::<Database>()
        }

        async fn resolve(&self, _ctx: Execution) -> Result<Box<dyn Any + Send>, ResolutionError> {
            Ok(Box::new(()))
        }
    }

    #[tokio::test]
    async fn a_value_of_another_type_is_a_type_mismatch() {
        let provider = MongoHealthProvider {
            connection: Arc::new(Box::new(Mistyped)),
        };
        match provider.resolve(Execution::None).await {
            Err(ResolutionError::TypeMismatch { token }) => {
                assert_eq!(token, ulo::di::token_of::<Database>())
            }
            Ok(_) => panic!("a value of another type is not the connection"),
            Err(other) => panic!("unexpected error: {other}"),
        }
    }
}
