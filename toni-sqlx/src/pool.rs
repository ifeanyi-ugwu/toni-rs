use std::{any::Any, marker::PhantomData, sync::Arc};

use async_trait::async_trait;
use sqlx::{Database, Pool, pool::PoolOptions};
use toni::{
    FxHashMap, StartupCheck,
    traits_helpers::{Injectable, Provider, ProviderContext, ProviderFactory},
};

pub(crate) struct SqlxPoolFactory<DB: Database> {
    pub url: String,
    // Injection token for this pool: the `Pool<DB>` type name for the default
    // (`for_root_*`), or the caller's chosen name for a `for_root_*_named` pool.
    pub token: String,
    pub check: Option<StartupCheck>,
    pub _db: PhantomData<DB>,
}

// PhantomData<DB> is not Send/Sync by default, so we assert it manually.
// Pool<DB> itself is Send+Sync for all sqlx DB types, and we only hold a String + marker.
unsafe impl<DB: Database> Send for SqlxPoolFactory<DB> {}
unsafe impl<DB: Database> Sync for SqlxPoolFactory<DB> {}

#[async_trait]
impl<DB> ProviderFactory for SqlxPoolFactory<DB>
where
    DB: Database + Send + Sync + 'static,
    for<'a> &'a mut DB::Connection: sqlx::Executor<'a, Database = DB>,
    for<'q> <DB as Database>::Arguments<'q>: sqlx::IntoArguments<'q, DB>,
    Pool<DB>: Send + Sync + Clone + 'static,
{
    fn get_token(&self) -> String {
        self.token.clone()
    }

    fn identity_hint(&self) -> Option<String> {
        Some(self.url.clone())
    }

    async fn build(
        &self,
        _deps: FxHashMap<String, toni::traits_helpers::Injectable>,
    ) -> Injectable {
        // Configured lazily: the server is contacted by the startup check, so every integration
        // reaches an unreachable one on the same schedule rather than on its driver's. What is
        // left here is URL parsing, which needs no network.
        //
        // `build` returns the instance directly, so a failure is carried into the provider and
        // reported from `on_module_init`, which can return it.
        let mut options = PoolOptions::<DB>::new();
        if let Some(check) = &self.check {
            options = options.acquire_timeout(check.attempt_timeout());
        }
        let (pool, init_error) = match options.connect_lazy(&self.url) {
            Ok(pool) => (Some(pool), None),
            Err(e) => (
                None,
                Some(crate::redact::describe(
                    "failed to configure the pool",
                    e,
                    &self.url,
                )),
            ),
        };

        Injectable::new(
            Arc::new(Box::new(SqlxPoolProvider {
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

struct SqlxPoolProvider<DB: Database> {
    pool: Option<Pool<DB>>,
    // Set when the pool could not be configured. `on_module_init` returns it, so startup stops
    // before anything resolves this provider.
    init_error: Option<String>,
    // `None` when the caller dropped the check: nothing contacts the server before it is used.
    check: Option<StartupCheck>,
    url: String,
    token: String,
}

unsafe impl<DB: Database> Send for SqlxPoolProvider<DB> {}
unsafe impl<DB: Database> Sync for SqlxPoolProvider<DB> {}

#[async_trait]
impl<DB> Provider for SqlxPoolProvider<DB>
where
    DB: Database + Send + Sync + 'static,
    for<'c> &'c mut DB::Connection: sqlx::Executor<'c, Database = DB>,
    for<'q> <DB as Database>::Arguments<'q>: sqlx::IntoArguments<'q, DB>,
    Pool<DB>: Send + Sync + Clone + 'static,
{
    fn get_token(&self) -> String {
        self.token.clone()
    }

    async fn execute(
        &self,
        _params: Vec<Box<dyn Any + Send>>,
        _ctx: ProviderContext,
    ) -> Box<dyn Any + Send> {
        // Pool<DB> is Arc-backed; cloning is cheap and shares the same connection pool.
        Box::new(self.pool.clone().expect("database pool unavailable"))
    }
    async fn on_module_init(&self) -> toni::InitResult {
        if let Some(message) = &self.init_error {
            return Err(message.clone().into());
        }

        let Some(check) = &self.check else {
            return Ok(());
        };

        let pool = self
            .pool
            .clone()
            .expect("a configured pool is present whenever there is no init error");

        check
            .run(
                || {
                    let pool = pool.clone();
                    async move {
                        sqlx::query("SELECT 1")
                            .execute(&pool)
                            .await
                            .map(|_| ())
                            .map_err(|e| {
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
            pool.close().await;
        }
    }
}
