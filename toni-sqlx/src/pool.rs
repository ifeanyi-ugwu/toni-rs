use std::{any::Any, marker::PhantomData, sync::Arc};

use async_trait::async_trait;
use sqlx::{Database, Pool};
use toni::{
    FxHashMap,
    traits_helpers::{Injectable, Provider, ProviderContext, ProviderFactory},
};

pub(crate) struct SqlxPoolFactory<DB: Database> {
    pub url: String,
    // Injection token for this pool: the `Pool<DB>` type name for the default
    // (`for_root_*`), or the caller's chosen name for a `for_root_*_named` pool.
    pub token: String,
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
        // `build` returns the instance directly, so a failed connection is carried into the
        // provider and reported from `on_module_init`, which can return it.
        let (pool, init_error) = match Pool::<DB>::connect(&self.url).await {
            Ok(pool) => (Some(pool), None),
            Err(e) => (
                None,
                Some(crate::redact::describe("failed to connect", e, &self.url)),
            ),
        };

        Injectable::new(
            Arc::new(Box::new(SqlxPoolProvider {
                pool,
                init_error,
                token: self.token.clone(),
            })),
            vec![],
        )
    }
}

struct SqlxPoolProvider<DB: Database> {
    pool: Option<Pool<DB>>,
    // Set when the connection could not be established. `on_module_init` returns it, so startup
    // stops before anything resolves this provider.
    init_error: Option<String>,
    token: String,
}

unsafe impl<DB: Database> Send for SqlxPoolProvider<DB> {}
unsafe impl<DB: Database> Sync for SqlxPoolProvider<DB> {}

#[async_trait]
impl<DB> Provider for SqlxPoolProvider<DB>
where
    DB: Database + Send + Sync + 'static,
    Pool<DB>: Send + Sync + Clone + 'static,
{
    fn get_token(&self) -> String {
        self.token.clone()
    }

    fn get_token_factory(&self) -> String {
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
        match &self.init_error {
            Some(message) => Err(message.clone().into()),
            None => Ok(()),
        }
    }

    async fn on_application_shutdown(&self, _signal: Option<String>) {
        if let Some(pool) = &self.pool {
            pool.close().await;
        }
    }
}
