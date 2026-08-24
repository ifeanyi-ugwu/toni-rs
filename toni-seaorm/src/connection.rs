use std::{any::Any, sync::Arc};

use async_trait::async_trait;
use parking_lot::Mutex;
use sea_orm::{Database, DatabaseConnection};
use toni::{
    FxHashMap,
    traits_helpers::{Provider, ProviderContext, ProviderFactory},
};

pub(crate) struct SeaOrmConnectionFactory {
    pub database_url: String,
    // Injection token for this connection: the `DatabaseConnection` type name for the default
    // (`for_root`), or the caller's chosen name for a `for_root_named` connection.
    pub token: String,
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
        // `build` returns the instance directly, so a failed connection is carried into the
        // provider and reported from `on_module_init`, which can return it.
        let (db, init_error) = match Database::connect(&self.database_url).await {
            Ok(db) => (Some(db), None),
            Err(e) => (
                None,
                Some(crate::redact::describe(
                    "failed to connect",
                    e,
                    &self.database_url,
                )),
            ),
        };

        toni::traits_helpers::Injectable::new(
            Arc::new(Box::new(SeaOrmConnectionProvider {
                db: Mutex::new(db),
                init_error,
                token: self.token.clone(),
            })),
            vec![],
        )
    }
}

struct SeaOrmConnectionProvider {
    // Option so close() can take ownership on shutdown; Mutex for &self access.
    db: Mutex<Option<DatabaseConnection>>,
    // Set when the connection could not be established. `on_module_init` returns it, so startup
    // stops before anything resolves this provider.
    init_error: Option<String>,
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
        match &self.init_error {
            Some(message) => Err(message.clone().into()),
            None => Ok(()),
        }
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
