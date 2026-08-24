#[cfg(any(feature = "postgres", feature = "mysql"))]
use std::{any::Any, sync::Arc};

#[cfg(any(feature = "postgres", feature = "mysql"))]
use async_trait::async_trait;
#[cfg(any(feature = "postgres", feature = "mysql"))]
use futures::future::BoxFuture;
#[cfg(any(feature = "postgres", feature = "mysql"))]
use toni::{
    FxHashMap,
    traits_helpers::{Injectable, Provider, ProviderContext, ProviderFactory},
};
#[cfg(any(feature = "postgres", feature = "mysql"))]
use toni_terminus::{HealthEntry, HealthIndicator, HealthIndicatorResult};

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
            fn get_token(&self) -> String {
                std::any::type_name::<$indicator>().to_string()
            }

            fn get_dependencies(&self) -> Vec<String> {
                vec![std::any::type_name::<$pool>().to_string()]
            }

            async fn build(&self, deps: FxHashMap<String, Injectable>) -> Injectable {
                let token = std::any::type_name::<$pool>().to_string();
                let connection = deps
                    .get(&token)
                    .expect("the health indicator is registered alongside the pool it checks")
                    .instance
                    .clone();
                Injectable::new(Arc::new(Box::new($provider { connection })), vec![])
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
            fn get_token(&self) -> String {
                std::any::type_name::<$indicator>().to_string()
            }

            fn get_token_factory(&self) -> String {
                std::any::type_name::<$indicator>().to_string()
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
                let pool = *resolved
                    .downcast::<$pool>()
                    .expect("the registered pool provider yields a Pool");
                Box::new($indicator { pool })
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
