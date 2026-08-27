#[cfg(any(feature = "postgres", feature = "mysql"))]
use std::{any::Any, sync::Arc};

#[cfg(any(feature = "postgres", feature = "mysql"))]
use async_trait::async_trait;
#[cfg(any(feature = "postgres", feature = "mysql"))]
use toni::{
    FxHashMap, StartupCheck,
    traits_helpers::{Injectable, Provider, ProviderContext, ProviderFactory},
};

#[cfg(any(feature = "postgres", feature = "mysql"))]
macro_rules! impl_diesel_pool {
    ($factory:ident, $provider:ident, $conn:ty, $pool:ty) => {
        pub(crate) struct $factory {
            pub url: String,
            // Injection token for this pool: the `Pool<_>` type name for the default
            // (`postgres`/`mysql`), or the caller's chosen name for a named pool.
            pub token: String,
            pub check: Option<StartupCheck>,
        }

        #[async_trait]
        impl ProviderFactory for $factory {
            fn get_token(&self) -> String {
                self.token.clone()
            }

            fn identity_hint(&self) -> Option<String> {
                Some(self.url.clone())
            }

            async fn build(&self, _deps: FxHashMap<String, Injectable>) -> Injectable {
                use diesel_async::pooled_connection::AsyncDieselConnectionManager;
                let manager = AsyncDieselConnectionManager::<$conn>::new(&self.url);
                // `build` returns the instance directly, so a failure is carried into the
                // provider and reported from `on_module_init`, which can return it. deadpool
                // opens no connection here, so only the pool configuration is checked.
                // deadpool returns a failed `create` rather than retrying it, so an attempt is
                // already bounded by the connection attempt itself and the check supplies the
                // retry.
                let (pool, init_error) = match <$pool>::builder(manager).build() {
                    Ok(pool) => (Some(pool), None),
                    Err(e) => (
                        None,
                        Some(crate::redact::describe(
                            "failed to build pool",
                            e,
                            &self.url,
                        )),
                    ),
                };
                Injectable::new(
                    Arc::new(Box::new($provider {
                        pool,
                        init_error,
                        check: self.check.clone(),
                        url: self.url.clone(),
                        token: self.token.clone(),
                    })),
                    vec![],
                )
            }
        }

        struct $provider {
            pool: Option<$pool>,
            // Set when the pool could not be built. `on_module_init` returns it, so startup
            // stops before anything resolves this provider.
            init_error: Option<String>,
            // `None` when the caller dropped the check: nothing contacts the server before it
            // is used.
            check: Option<StartupCheck>,
            url: String,
            token: String,
        }

        #[async_trait]
        impl Provider for $provider {
            fn get_token(&self) -> String {
                self.token.clone()
            }

            async fn execute(
                &self,
                _params: Vec<Box<dyn Any + Send>>,
                _ctx: ProviderContext,
            ) -> Box<dyn Any + Send> {
                Box::new(self.pool.clone().expect("database pool unavailable"))
            }

            async fn on_module_init(&self) -> toni::InitResult {
                if let Some(message) = &self.init_error {
                    return Err(message.clone().into());
                }

                let Some(check) = &self.check else {
                    return Ok(());
                };

                // deadpool opens no connection when it builds, so taking one from the pool is
                // what makes an unreachable server a startup failure rather than an error on
                // the first query.
                let pool = self
                    .pool
                    .clone()
                    .expect("a built pool is present whenever there is no init error");

                check
                    .run(
                        || {
                            let pool = pool.clone();
                            async move {
                                pool.get().await.map(|_| ()).map_err(|e| {
                                    crate::redact::describe(
                                        "failed to reach the database",
                                        e,
                                        &self.url,
                                    )
                                })
                            }
                        },
                        futures_timer::Delay::new,
                    )
                    .await
                    .map_err(Into::into)
            }

            async fn on_application_shutdown(&self, _signal: Option<String>) {
                if let Some(pool) = &self.pool {
                    pool.close();
                }
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
