use std::{any::Any, marker::PhantomData, sync::Arc};

use async_trait::async_trait;
use sqlx::{Database, Pool};
use toni::{
    FxHashMap,
    traits_helpers::{Provider, ProviderContext, ProviderFactory, ProviderRole},
};

pub(crate) struct SqlxPoolFactory<DB: Database> {
    pub url: String,
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
        std::any::type_name::<Pool<DB>>().to_string()
    }

    async fn build(
        &self,
        _deps: FxHashMap<String, (Arc<Box<dyn Provider>>, Vec<toni::traits_helpers::ProviderRole>)>,
    ) -> (Arc<Box<dyn Provider>>, Vec<ProviderRole>) {
        let pool = Pool::<DB>::connect(&self.url)
            .await
            .unwrap_or_else(|e| panic!("toni-sqlx: failed to connect to '{}': {e}", self.url));

        (Arc::new(Box::new(SqlxPoolProvider { pool })), vec![])
    }
}

struct SqlxPoolProvider<DB: Database> {
    pool: Pool<DB>,
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
        std::any::type_name::<Pool<DB>>().to_string()
    }

    fn get_token_factory(&self) -> String {
        std::any::type_name::<Pool<DB>>().to_string()
    }

    async fn execute(
        &self,
        _params: Vec<Box<dyn Any + Send>>,
        _ctx: ProviderContext<'_>,
    ) -> Box<dyn Any + Send> {
        // Pool<DB> is Arc-backed; cloning is cheap and shares the same connection pool.
        Box::new(self.pool.clone())
    }

    async fn on_application_shutdown(&self, _signal: Option<String>) {
        self.pool.close().await;
    }
}
