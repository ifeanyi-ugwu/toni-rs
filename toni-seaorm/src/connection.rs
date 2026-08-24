use std::{any::Any, sync::Arc};

use async_trait::async_trait;
use parking_lot::Mutex;
use sea_orm::{ConnectOptions, Database, DatabaseConnection};
use toni::{
    FxHashMap, StartupCheck,
    traits_helpers::{Provider, ProviderContext, ProviderFactory},
};

pub(crate) struct SeaOrmConnectionFactory {
    pub database_url: String,
    // Injection token for this connection: the `DatabaseConnection` type name for the default
    // (`for_root`), or the caller's chosen name for a `for_root_named` connection.
    pub token: String,
    pub check: Option<StartupCheck>,
}

#[async_trait]
impl ProviderFactory for SeaOrmConnectionFactory {
    fn get_token(&self) -> String {
        self.token.clone()
    }

    fn identity_hint(&self) -> Option<String> {
        Some(self.database_url.clone())
    }

    async fn build(
        &self,
        _deps: FxHashMap<String, toni::traits_helpers::Injectable>,
    ) -> toni::traits_helpers::Injectable {
        // Configured lazily, with the check's deadline handed to the driver: sea-orm's own
        // connect and acquire timeouts are what bound the probe, so nothing here needs a timer.
        // What is left at build time is URL parsing, which needs no network.
        let mut options = ConnectOptions::new(&self.database_url);
        options.connect_lazy(true);
        if let Some(check) = &self.check {
            options
                .connect_timeout(check.attempt_timeout())
                .acquire_timeout(check.attempt_timeout());
        }

        // `build` returns the instance directly, so a failure is carried into the provider and
        // reported from `on_module_init`, which can return it.
        let (db, init_error) = match Database::connect(options).await {
            Ok(db) => (Some(db), None),
            Err(e) => (
                None,
                Some(crate::redact::describe(
                    "failed to configure the connection",
                    e,
                    &self.database_url,
                )),
            ),
        };

        toni::traits_helpers::Injectable::new(
            Arc::new(Box::new(SeaOrmConnectionProvider {
                db: Mutex::new(db),
                init_error,
                check: self.check.clone(),
                database_url: self.database_url.clone(),
                token: self.token.clone(),
            })),
            vec![],
        )
    }
}

struct SeaOrmConnectionProvider {
    // Option so close() can take ownership on shutdown; Mutex for &self access.
    db: Mutex<Option<DatabaseConnection>>,
    // Set when the pool could not be configured. `on_module_init` returns it, so startup stops
    // before anything resolves this provider.
    init_error: Option<String>,
    // `None` when the caller dropped the check: nothing contacts the server before it is used.
    check: Option<StartupCheck>,
    database_url: String,
    token: String,
}

#[async_trait]
impl Provider for SeaOrmConnectionProvider {
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
        // DatabaseConnection is Clone — it wraps a connection pool internally.
        let db = self
            .db
            .lock()
            .as_ref()
            .expect("database connection unavailable")
            .clone();
        Box::new(db)
    }

    async fn on_module_init(&self) -> toni::InitResult {
        if let Some(message) = &self.init_error {
            return Err(message.clone().into());
        }

        let Some(check) = &self.check else {
            return Ok(());
        };

        let db = self
            .db
            .lock()
            .as_ref()
            .expect("a configured pool is present whenever there is no init error")
            .clone();

        // Each attempt is bounded by the acquire timeout handed to the driver at build time; the
        // gaps between them are this check's, so every integration gives up at the same point.
        check
            .run(
                || {
                    let db = db.clone();
                    async move {
                        db.ping().await.map_err(|e| {
                            crate::redact::describe(
                                "failed to reach the database",
                                e,
                                &self.database_url,
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
        let db = self.db.lock().take();
        if let Some(db) = db {
            if let Err(e) = db.close().await {
                tracing::error!(error = %e, "SeaORM: error closing database connection");
            }
        }
    }
}
