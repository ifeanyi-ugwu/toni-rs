use std::{any::Any, sync::Arc};

use async_trait::async_trait;
use mongodb::{Client, Database, options::ClientOptions};
use toni::{
    FxHashMap, StartupCheck,
    traits_helpers::{Provider, ProviderContext, ProviderFactory},
};

pub(crate) struct MongoConnectionFactory {
    pub uri: String,
    pub db_name: String,
    // Injection token for this connection: the `Database` type name for the default
    // (`for_root`), or the caller's chosen name for a `for_root_named` connection.
    pub token: String,
    pub check: Option<StartupCheck>,
}

#[async_trait]
impl ProviderFactory for MongoConnectionFactory {
    fn get_token(&self) -> String {
        self.token.clone()
    }

    fn identity_hint(&self) -> Option<String> {
        Some(format!("{}/{}", self.uri, self.db_name))
    }

    async fn build(
        &self,
        _deps: FxHashMap<String, toni::traits_helpers::Injectable>,
    ) -> toni::traits_helpers::Injectable {
        // `build` returns the instance directly, so a failure is carried into the provider and
        // reported from `on_module_init`, which can return it. The driver connects lazily, so
        // only URI parsing and client construction are checked here.
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

        toni::traits_helpers::Injectable::new(
            Arc::new(Box::new(MongoConnectionProvider {
                client,
                db,
                init_error,
                check: self.check.clone(),
                uri: self.uri.clone(),
                token: self.token.clone(),
            })),
            vec![],
        )
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
        // Database is Clone (Arc-backed); cloning shares the same connection pool.
        Box::new(self.db.clone().expect("mongo database unavailable"))
    }

    async fn on_module_init(&self) -> toni::InitResult {
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
