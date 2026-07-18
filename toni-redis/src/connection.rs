use std::{any::Any, sync::Arc};

use async_trait::async_trait;
use redis::aio::ConnectionManager;
use toni::{
    FxHashMap,
    traits_helpers::{Provider, ProviderContext, ProviderFactory},
};

pub(crate) struct RedisConnectionFactory {
    pub url: String,
    // Injection token for this connection: the `ConnectionManager` type name for the default
    // (`for_root`), or the caller's chosen name for a `for_root_named` connection.
    pub token: String,
}

#[async_trait]
impl ProviderFactory for RedisConnectionFactory {
    fn get_token(&self) -> String {
        self.token.clone()
    }

    fn identity_hint(&self) -> Option<String> {
        Some(self.url.clone())
    }

    async fn build(
        &self,
        _deps: FxHashMap<String, toni::traits_helpers::Injectable>,
    ) -> toni::traits_helpers::Injectable {
        let client = redis::Client::open(self.url.as_str())
            .unwrap_or_else(|e| panic!("toni-redis: invalid URL '{}': {e}", self.url));

        let manager = ConnectionManager::new(client)
            .await
            .unwrap_or_else(|e| panic!("toni-redis: failed to connect to '{}': {e}", self.url));

        toni::traits_helpers::Injectable::new(
            Arc::new(Box::new(RedisConnectionProvider {
                manager,
                token: self.token.clone(),
            })),
            vec![],
        )
    }
}

struct RedisConnectionProvider {
    manager: ConnectionManager,
    token: String,
}

#[async_trait]
impl Provider for RedisConnectionProvider {
    fn get_token(&self) -> String {
        self.token.clone()
    }

    fn get_token_factory(&self) -> String {
        self.token.clone()
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
