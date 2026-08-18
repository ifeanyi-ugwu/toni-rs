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

        pub(crate) struct $factory {
            pub url: String,
        }

        #[async_trait]
        impl ProviderFactory for $factory {
            fn get_token(&self) -> String {
                std::any::type_name::<$indicator>().to_string()
            }

            async fn build(&self, _deps: FxHashMap<String, Injectable>) -> Injectable {
                use diesel_async::pooled_connection::AsyncDieselConnectionManager;
                let manager = AsyncDieselConnectionManager::<$conn>::new(&self.url);
                let pool = <$pool>::builder(manager).build().unwrap_or_else(|e| {
                    panic!(
                        "toni-diesel health: failed to build pool for '{}': {e}",
                        self.url
                    )
                });
                Injectable::new(
                    Arc::new(Box::new($provider {
                        indicator: $indicator { pool },
                    })),
                    vec![],
                )
            }
        }

        struct $provider {
            indicator: $indicator,
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
                Box::new(self.indicator.clone())
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
