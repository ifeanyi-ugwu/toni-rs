#[cfg(any(feature = "postgres", feature = "mysql"))]
use std::{any::Any, sync::Arc};

#[cfg(any(feature = "postgres", feature = "mysql"))]
use async_trait::async_trait;
#[cfg(any(feature = "postgres", feature = "mysql"))]
use futures::future::BoxFuture;
#[cfg(any(feature = "postgres", feature = "mysql"))]
use ulo::{
    FxHashMap,
    di::{Execution, ResolutionError},
    spi::{BuildResult, Injectable, Provider, ProviderFactory},
};
#[cfg(any(feature = "postgres", feature = "mysql"))]
use ulo_health::{HealthEntry, HealthIndicator, HealthIndicatorResult};

#[cfg(any(feature = "postgres", feature = "mysql"))]
macro_rules! impl_diesel_health {
    ($indicator:ident, $factory:ident, $provider:ident, $conn:ty, $pool:ty) => {
        #[derive(Clone)]
        pub struct $indicator {
            pool: $pool,
        }

        impl $indicator {
            pub fn ping_check(&self, key: &str) -> BoxFuture<'static, HealthIndicatorResult> {
                let key = key.to_string();
                let pool = self.pool.clone();
                Box::pin(async move {
                    let mut conn = match pool.get().await {
                        Ok(c) => c,
                        Err(e) => {
                            return Err(HealthEntry::down_with(
                                key,
                                serde_json::json!({ "message": e.to_string() }),
                            ));
                        }
                    };
                    use diesel::sql_query;
                    use diesel_async::RunQueryDsl;
                    match sql_query("SELECT 1").execute(&mut *conn).await {
                        Ok(_) => Ok(HealthEntry::up(key)),
                        Err(e) => Err(HealthEntry::down_with(
                            key,
                            serde_json::json!({ "message": e.to_string() }),
                        )),
                    }
                })
            }
        }

        impl HealthIndicator for $indicator {
            fn check(&self, key: &str) -> BoxFuture<'static, HealthIndicatorResult> {
                self.ping_check(key)
            }
        }

        pub(crate) struct $factory;

        #[async_trait]
        impl ProviderFactory for $factory {
            fn token(&self) -> String {
                ulo::di::token_of::<$indicator>()
            }

            fn dependency_tokens(&self) -> Vec<String> {
                vec![ulo::di::token_of::<$pool>()]
            }

            async fn build(&self, deps: FxHashMap<String, Injectable>) -> BuildResult<Injectable> {
                let token = ulo::di::token_of::<$pool>();
                let connection = deps
                    .get(&token)
                    .expect("the health indicator is registered alongside the pool it checks")
                    .instance
                    .clone();
                Ok(Injectable::new(Arc::new(Box::new($provider { connection })), vec![]))
            }
        }

        struct $provider {
            // The registered pool's provider, resolved per request for an indicator rather than
            // at build time: the pool may have failed, and startup reports that from its own
            // `on_module_init` before anything can resolve this one.
            connection: Arc<Box<dyn Provider>>,
        }

        #[async_trait]
        impl Provider for $provider {
            fn token(&self) -> String {
                ulo::di::token_of::<$indicator>()
            }


            async fn resolve(
                &self,
                _ctx: Execution,
            ) -> Result<Box<dyn Any + Send>, ResolutionError> {
                let resolved = self
                    .connection
                    .resolve(Execution::None)
                    .await?;
                let pool = *resolved
                    .downcast::<$pool>()
                    .expect("the registered pool provider yields a Pool");
                Ok(Box::new($indicator { pool }))
            }
        }
    };
}

#[cfg(feature = "postgres")]
impl_diesel_health!(
    PgHealthIndicator,
    PgHealthIndicatorFactory,
    PgHealthProvider,
    diesel_async::AsyncPgConnection,
    diesel_async::pooled_connection::deadpool::Pool<diesel_async::AsyncPgConnection>
);

#[cfg(feature = "mysql")]
impl_diesel_health!(
    MySqlHealthIndicator,
    MySqlHealthIndicatorFactory,
    MySqlHealthProvider,
    diesel_async::AsyncMysqlConnection,
    diesel_async::pooled_connection::deadpool::Pool<diesel_async::AsyncMysqlConnection>
);
