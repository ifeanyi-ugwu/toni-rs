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
        // `build` returns the instance directly, so a failed connection is carried into the
        // provider and reported from `on_module_init`, which can return it.
        let (manager, init_error) = match redis::Client::open(self.url.as_str()) {
            Err(e) => (
                None,
                Some(crate::redact::describe("invalid URL", e, &self.url)),
            ),
            Ok(client) => match ConnectionManager::new(client).await {
                Ok(manager) => (Some(manager), None),
                Err(e) => (
                    None,
                    Some(crate::redact::describe("failed to connect", e, &self.url)),
                ),
            },
        };

        toni::traits_helpers::Injectable::new(
            Arc::new(Box::new(RedisConnectionProvider {
                manager,
                init_error,
                token: self.token.clone(),
            })),
            vec![],
        )
    }
}

struct RedisConnectionProvider {
    manager: Option<ConnectionManager>,
    // Set when the connection could not be established. `on_module_init` returns it, so startup
    // stops before anything resolves this provider.
    init_error: Option<String>,
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
        _ctx: ProviderContext,
    ) -> Box<dyn Any + Send> {
        // ConnectionManager is Clone (Arc-backed); clones share the same underlying connection.
        Box::new(self.manager.clone().expect("redis connection unavailable"))
    }
    async fn on_module_init(&self) -> toni::InitResult {
        match &self.init_error {
            Some(message) => Err(message.clone().into()),
            None => Ok(()),
        }
    }
}
