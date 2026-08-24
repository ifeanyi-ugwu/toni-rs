use std::{any::Any, marker::PhantomData, sync::Arc};

use async_trait::async_trait;
use futures::future::BoxFuture;
use sqlx::{Database, Pool};
use toni::{
    FxHashMap,
    traits_helpers::{Injectable, Provider, ProviderContext, ProviderFactory},
};
use toni_terminus::{HealthEntry, HealthIndicator, HealthIndicatorResult};

pub struct SqlxHealthIndicator<DB: Database> {
    pool: Pool<DB>,
}

// Pool<DB> is Send+Sync for all sqlx DB types; PhantomData<DB> is not automatically.
unsafe impl<DB: Database> Send for SqlxHealthIndicator<DB> {}
unsafe impl<DB: Database> Sync for SqlxHealthIndicator<DB> {}

impl<DB: Database> Clone for SqlxHealthIndicator<DB> {
    fn clone(&self) -> Self {
        Self {
            pool: self.pool.clone(),
        }
    }
}

impl<DB> SqlxHealthIndicator<DB>
where
    DB: Database + Send + Sync + 'static,
    for<'c> &'c mut DB::Connection: sqlx::Executor<'c, Database = DB>,
    for<'q> <DB as Database>::Arguments<'q>: sqlx::IntoArguments<'q, DB>,
    Pool<DB>: Send + Sync + Clone + 'static,
{
    pub fn ping_check(&self, key: &str) -> BoxFuture<'static, HealthIndicatorResult> {
        let key = key.to_string();
        let pool = self.pool.clone();
        Box::pin(async move {
            match sqlx::query("SELECT 1").execute(&pool).await {
                Ok(_) => Ok(HealthEntry::up(key)),
                Err(e) => Err(HealthEntry::down_with(
                    key,
                    serde_json::json!({ "message": e.to_string() }),
                )),
            }
        })
    }
}

impl<DB> HealthIndicator for SqlxHealthIndicator<DB>
where
    DB: Database + Send + Sync + 'static,
    for<'c> &'c mut DB::Connection: sqlx::Executor<'c, Database = DB>,
    for<'q> <DB as Database>::Arguments<'q>: sqlx::IntoArguments<'q, DB>,
    Pool<DB>: Send + Sync + Clone + 'static,
{
    fn check(&self, key: &str) -> BoxFuture<'static, HealthIndicatorResult> {
        self.ping_check(key)
    }
}

// ── DI machinery ─────────────────────────────────────────────────────────────

pub(crate) struct SqlxHealthIndicatorFactory<DB: Database> {
    pub _db: PhantomData<DB>,
}

unsafe impl<DB: Database> Send for SqlxHealthIndicatorFactory<DB> {}
unsafe impl<DB: Database> Sync for SqlxHealthIndicatorFactory<DB> {}

#[async_trait]
impl<DB> ProviderFactory for SqlxHealthIndicatorFactory<DB>
where
    DB: Database + Send + Sync + 'static,
    for<'c> &'c mut DB::Connection: sqlx::Executor<'c, Database = DB>,
    for<'q> <DB as Database>::Arguments<'q>: sqlx::IntoArguments<'q, DB>,
    Pool<DB>: Send + Sync + Clone + 'static,
{
    fn get_token(&self) -> String {
        std::any::type_name::<SqlxHealthIndicator<DB>>().to_string()
    }

    fn get_dependencies(&self) -> Vec<String> {
        vec![std::any::type_name::<Pool<DB>>().to_string()]
    }

    async fn build(&self, deps: FxHashMap<String, Injectable>) -> Injectable {
        let token = std::any::type_name::<Pool<DB>>().to_string();
        let connection = deps
            .get(&token)
            .expect("the health indicator is registered alongside the pool it checks")
            .instance
            .clone();
        Injectable::new(
            Arc::new(Box::new(SqlxHealthProvider::<DB> {
                connection,
                _db: PhantomData,
            })),
            vec![],
        )
    }
}

struct SqlxHealthProvider<DB: Database> {
    // The registered pool's provider, resolved per request for an indicator rather than at build
    // time: the pool may have failed, and startup reports that from its own `on_module_init`
    // before anything can resolve this one.
    connection: Arc<Box<dyn Provider>>,
    _db: PhantomData<DB>,
}

unsafe impl<DB: Database> Send for SqlxHealthProvider<DB> {}
unsafe impl<DB: Database> Sync for SqlxHealthProvider<DB> {}

#[async_trait]
impl<DB> Provider for SqlxHealthProvider<DB>
where
    DB: Database + Send + Sync + 'static,
    for<'c> &'c mut DB::Connection: sqlx::Executor<'c, Database = DB>,
    for<'q> <DB as Database>::Arguments<'q>: sqlx::IntoArguments<'q, DB>,
    Pool<DB>: Send + Sync + Clone + 'static,
{
    fn get_token(&self) -> String {
        std::any::type_name::<SqlxHealthIndicator<DB>>().to_string()
    }

    fn get_token_factory(&self) -> String {
        std::any::type_name::<SqlxHealthIndicator<DB>>().to_string()
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
            .downcast::<Pool<DB>>()
            .expect("the registered pool provider yields a Pool");
        Box::new(SqlxHealthIndicator { pool })
    }
}
