use std::{any::Any, sync::Arc};

use async_trait::async_trait;
use redis::aio::ConnectionManager;
use toni::{
    FxHashMap,
    traits_helpers::{Provider, ProviderContext, ProviderFactory},
};

pub(crate) struct RedisConnectionFactory {
    pub url: String,
}

#[async_trait]
impl ProviderFactory for RedisConnectionFactory {
    fn get_token(&self) -> String {
        std::any::type_name::<ConnectionManager>().to_string()
    }

    async fn build(
        &self,
        _deps: FxHashMap<String, Arc<Box<dyn Provider>>>,
    ) -> (Arc<Box<dyn Provider>>, Vec<toni::traits_helpers::ProviderRole>) {
        let client = redis::Client::open(self.url.as_str())
            .unwrap_or_else(|e| panic!("toni-redis: invalid URL '{}': {e}", self.url));

        let manager = ConnectionManager::new(client)
            .await
            .unwrap_or_else(|e| panic!("toni-redis: failed to connect to '{}': {e}", self.url));

        (Arc::new(Box::new(RedisConnectionProvider { manager })), vec![])
    }
}

struct RedisConnectionProvider {
    manager: ConnectionManager,
}

#[async_trait]
impl Provider for RedisConnectionProvider {
    fn get_token(&self) -> String {
        std::any::type_name::<ConnectionManager>().to_string()
    }

    fn get_token_factory(&self) -> String {
        std::any::type_name::<ConnectionManager>().to_string()
    }

    async fn execute(
        &self,
        _params: Vec<Box<dyn Any + Send>>,
        _ctx: ProviderContext<'_>,
    ) -> Box<dyn Any + Send> {
        // ConnectionManager is Clone (Arc-backed); clones share the same underlying connection.
        Box::new(self.manager.clone())
    }
}
