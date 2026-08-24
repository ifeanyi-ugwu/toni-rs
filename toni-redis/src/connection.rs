use std::{any::Any, sync::Arc};

use async_trait::async_trait;
use redis::aio::{ConnectionManager, ConnectionManagerConfig};
use toni::{
    FxHashMap, StartupCheck,
    traits_helpers::{Provider, ProviderContext, ProviderFactory},
};

pub(crate) struct RedisConnectionFactory {
    pub url: String,
    // Injection token for this connection: the `ConnectionManager` type name for the default
    // (`for_root`), or the caller's chosen name for a `for_root_named` connection.
    pub token: String,
    pub check: Option<StartupCheck>,
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
        // Configured lazily, with the check's deadline handed to the driver: its own connection
        // timeout is what bounds the probe, so nothing here needs a timer.
        // The driver's own retry is switched off and each attempt bounded, so the check's
        // schedule is the only one: left on, its six attempts with exponential backoff overrun
        // any deadline set here.
        let mut config = ConnectionManagerConfig::new();
        if let Some(check) = &self.check {
            config = config
                .set_number_of_retries(0)
                .set_connection_timeout(Some(check.attempt_timeout()));
        }

        // `build` returns the instance directly, so a failure is carried into the provider and
        // reported from `on_module_init`, which can return it.
        let (manager, init_error) = match redis::Client::open(self.url.as_str()) {
            Err(e) => (
                None,
                Some(crate::redact::describe("invalid URL", e, &self.url)),
            ),
            Ok(client) => match ConnectionManager::new_lazy_with_config(client, config) {
                Ok(manager) => (Some(manager), None),
                Err(e) => (
                    None,
                    Some(crate::redact::describe(
                        "failed to configure the connection",
                        e,
                        &self.url,
                    )),
                ),
            },
        };

        toni::traits_helpers::Injectable::new(
            Arc::new(Box::new(RedisConnectionProvider {
                manager,
                init_error,
                check: self.check.clone(),
                url: self.url.clone(),
                token: self.token.clone(),
            })),
            vec![],
        )
    }
}

struct RedisConnectionProvider {
    manager: Option<ConnectionManager>,
    // Set when the connection could not be configured. `on_module_init` returns it, so startup
    // stops before anything resolves this provider.
    init_error: Option<String>,
    // `None` when the caller dropped the check: nothing contacts the server before it is used.
    check: Option<StartupCheck>,
    url: String,
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
        if let Some(message) = &self.init_error {
            return Err(message.clone().into());
        }

        let Some(check) = &self.check else {
            return Ok(());
        };

        let manager = self
            .manager
            .clone()
            .expect("a configured manager is present whenever there is no init error");

        check
            .run(
                || {
                    let mut manager = manager.clone();
                    async move {
                        redis::cmd("PING")
                            .query_async::<String>(&mut manager)
                            .await
                            .map(|_| ())
                            .map_err(|e| {
                                crate::redact::describe("failed to reach the server", e, &self.url)
                            })
                    }
                },
                futures_timer::Delay::new,
            )
            .await
            .map_err(Into::into)
    }
}
