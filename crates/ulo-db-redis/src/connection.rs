use std::{any::Any, sync::Arc};

use async_trait::async_trait;
use redis::aio::{ConnectionManager, ConnectionManagerConfig};
use ulo::{
    FxHashMap, StartupCheck,
    di::{Execution, ResolutionError},
    spi::{BuildResult, Provider, ProviderFactory},
};

pub(crate) struct RedisConnectionFactory {
    pub url: String,
    // Injection token for this connection: `ConnectionManager`'s own for the default (`for_root`),
    // or the marker's for a `for_root_keyed` connection.
    pub token: String,
    pub check: Option<StartupCheck>,
}

#[async_trait]
impl ProviderFactory for RedisConnectionFactory {
    fn token(&self) -> String {
        self.token.clone()
    }

    fn identity_hint(&self) -> Option<String> {
        Some(self.url.clone())
    }

    async fn build(
        &self,
        _deps: FxHashMap<String, ulo::spi::Registration>,
    ) -> BuildResult<ulo::spi::Registration> {
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

        // A failure is carried into the provider and reported from `on_module_init` with the
        // startup check's, so either arrives as `StartupError::HookFailed`.
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

        Ok(ulo::spi::Registration::new(
            Arc::new(Box::new(RedisConnectionProvider {
                manager,
                init_error,
                check: self.check.clone(),
                url: self.url.clone(),
                token: self.token.clone(),
            })),
            vec![],
        ))
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
    fn token(&self) -> String {
        self.token.clone()
    }

    async fn resolve(&self, _ctx: Execution) -> Result<Box<dyn Any + Send>, ResolutionError> {
        // ConnectionManager is Clone (Arc-backed); clones share the same underlying connection.
        Ok(Box::new(
            self.manager.clone().expect("redis connection unavailable"),
        ))
    }
    async fn on_module_init(&self) -> ulo::di::InitResult {
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
