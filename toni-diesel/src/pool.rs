#[cfg(any(feature = "postgres", feature = "mysql"))]
use std::{any::Any, sync::Arc};

#[cfg(any(feature = "postgres", feature = "mysql"))]
use async_trait::async_trait;
#[cfg(any(feature = "postgres", feature = "mysql"))]
use toni::{
    FxHashMap,
    traits_helpers::{Provider, ProviderContext, ProviderFactory},
};

#[cfg(any(feature = "postgres", feature = "mysql"))]
macro_rules! impl_diesel_pool {
    ($factory:ident, $provider:ident, $conn:ty, $pool:ty) => {
        pub(crate) struct $factory {
            pub url: String,
        }

        #[async_trait]
        impl ProviderFactory for $factory {
            fn get_token(&self) -> String {
                std::any::type_name::<$pool>().to_string()
            }

            async fn build(
                &self,
                _deps: FxHashMap<String, Arc<Box<dyn Provider>>>,
            ) -> Arc<Box<dyn Provider>> {
                use diesel_async::pooled_connection::AsyncDieselConnectionManager;
                let manager = AsyncDieselConnectionManager::<$conn>::new(&self.url);
                let pool = <$pool>::builder(manager).build().unwrap_or_else(|e| {
                    panic!("toni-diesel: failed to build pool for '{}': {e}", self.url)
                });
                Arc::new(Box::new($provider { pool }))
            }
        }

        struct $provider {
            pool: $pool,
        }

        #[async_trait]
        impl Provider for $provider {
            fn get_token(&self) -> String {
                std::any::type_name::<$pool>().to_string()
            }

            fn get_token_factory(&self) -> String {
                std::any::type_name::<$pool>().to_string()
            }

            async fn execute(
                &self,
                _params: Vec<Box<dyn Any + Send>>,
                _ctx: ProviderContext<'_>,
            ) -> Box<dyn Any + Send> {
                Box::new(self.pool.clone())
            }

            async fn on_application_shutdown(&self, _signal: Option<String>) {
                self.pool.close();
            }
        }
    };
}

#[cfg(feature = "postgres")]
impl_diesel_pool!(
    PgPoolFactory,
    PgPoolProvider,
    diesel_async::AsyncPgConnection,
    diesel_async::pooled_connection::deadpool::Pool<diesel_async::AsyncPgConnection>
);

#[cfg(feature = "mysql")]
impl_diesel_pool!(
    MySqlPoolFactory,
    MySqlPoolProvider,
    diesel_async::AsyncMysqlConnection,
    diesel_async::pooled_connection::deadpool::Pool<diesel_async::AsyncMysqlConnection>
);
