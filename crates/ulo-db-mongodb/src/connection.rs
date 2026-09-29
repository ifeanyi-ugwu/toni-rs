use std::{any::Any, sync::Arc};

use async_trait::async_trait;
use mongodb::{Client, Database, options::ClientOptions};
use ulo::{
    FxHashMap, StartupCheck,
    di::{Execution, ResolutionError},
    spi::{BuildResult, Provider, ProviderFactory},
};

pub(crate) struct MongoConnectionFactory {
    pub uri: String,
    pub db_name: String,
    // Injection token for this connection: `Database`'s own for the default (`for_root`), or the
    // marker's for a `for_root_keyed` connection.
    pub token: String,
    pub check: Option<StartupCheck>,
}

#[async_trait]
impl ProviderFactory for MongoConnectionFactory {
    fn token(&self) -> String {
        self.token.clone()
    }

    fn identity_hint(&self) -> Option<String> {
        Some(format!("{}/{}", self.uri, self.db_name))
    }

    async fn build(
        &self,
        _deps: FxHashMap<String, ulo::spi::Registration>,
    ) -> BuildResult<ulo::spi::Registration> {
        // A failure is carried into the provider and reported from `on_module_init` with the
        // startup check's, so either arrives as `StartupError::HookFailed`. The driver connects
        // lazily, so only URI parsing and client construction are checked here.
        let (client, init_error) = match ClientOptions::parse(&self.uri).await.map(|mut o| {
            if let Some(check) = &self.check {
                o.server_selection_timeout = Some(check.attempt_timeout());
                o.connect_timeout = Some(check.attempt_timeout());
            }
            o
        }) {
            Err(e) => (
                None,
                Some(crate::redact::describe("failed to parse URI", e, &self.uri)),
            ),
            Ok(options) => match Client::with_options(options) {
                Ok(client) => (Some(client), None),
                Err(e) => (
                    None,
                    Some(crate::redact::describe(
                        "failed to create client",
                        e,
                        &self.uri,
                    )),
                ),
            },
        };
        let db = client.as_ref().map(|c| c.database(&self.db_name));

        Ok(ulo::spi::Registration::new(
            Arc::new(Box::new(MongoConnectionProvider {
                client,
                db,
                init_error,
                check: self.check.clone(),
                uri: self.uri.clone(),
                token: self.token.clone(),
            })),
            vec![],
        ))
    }
}

struct MongoConnectionProvider {
    // Held so shutdown() can be called; Client is Arc-backed and cheap to clone.
    client: Option<Client>,
    db: Option<Database>,
    // Set when the client could not be constructed. `on_module_init` returns it, so startup stops
    // before anything resolves this provider.
    init_error: Option<String>,
    // `None` when the caller dropped the check: nothing contacts the server before it is used.
    check: Option<StartupCheck>,
    uri: String,
    token: String,
}

#[async_trait]
impl Provider for MongoConnectionProvider {
    fn token(&self) -> String {
        self.token.clone()
    }

    async fn resolve(&self, _ctx: Execution) -> Result<Box<dyn Any + Send>, ResolutionError> {
        // Database is Clone (Arc-backed); cloning shares the same connection pool.
        Ok(Box::new(
            self.db.clone().expect("mongo database unavailable"),
        ))
    }

    async fn on_module_init(&self) -> ulo::di::InitResult {
        if let Some(message) = &self.init_error {
            return Err(message.clone().into());
        }

        let Some(check) = &self.check else {
            return Ok(());
        };

        // The driver connects on first use, so this ping is what makes an unreachable server a
        // startup failure rather than an error on the first query.
        let db = self
            .db
            .clone()
            .expect("a client is present whenever there is no init error");

        check
            .run(
                || {
                    let db = db.clone();
                    async move {
                        db.run_command(mongodb::bson::doc! { "ping": 1 })
                            .await
                            .map(|_| ())
                            .map_err(|e| {
                                crate::redact::describe("failed to reach the server", e, &self.uri)
                            })
                    }
                },
                futures_timer::Delay::new,
            )
            .await
            .map_err(Into::into)
    }

    async fn on_application_shutdown(&self, _signal: Option<String>) {
        if let Some(client) = self.client.clone() {
            client.shutdown().await;
        }
    }
}
