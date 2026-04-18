use std::{any::Any, sync::Arc};

use async_trait::async_trait;
use mongodb::{Client, Database, options::ClientOptions};
use toni::{
    FxHashMap,
    traits_helpers::{Provider, ProviderContext, ProviderFactory},
};

pub(crate) struct MongoConnectionFactory {
    pub uri: String,
    pub db_name: String,
}

#[async_trait]
impl ProviderFactory for MongoConnectionFactory {
    fn get_token(&self) -> String {
        std::any::type_name::<Database>().to_string()
    }

    async fn build(
        &self,
        _deps: FxHashMap<String, toni::traits_helpers::Injectable>,
    ) -> toni::traits_helpers::Injectable {
        let options = ClientOptions::parse(&self.uri)
            .await
            .unwrap_or_else(|e| panic!("toni-mongodb: failed to parse URI '{}': {e}", self.uri));

        let client = Client::with_options(options)
            .unwrap_or_else(|e| panic!("toni-mongodb: failed to create client: {e}"));

        let db = client.database(&self.db_name);

        toni::traits_helpers::Injectable::new(Arc::new(Box::new(MongoConnectionProvider { client, db })), vec![])
    }
}

struct MongoConnectionProvider {
    // Held so shutdown() can be called; Client is Arc-backed and cheap to clone.
    client: Client,
    db: Database,
}

#[async_trait]
impl Provider for MongoConnectionProvider {
    fn get_token(&self) -> String {
        std::any::type_name::<Database>().to_string()
    }

    fn get_token_factory(&self) -> String {
        std::any::type_name::<Database>().to_string()
    }

    async fn execute(
        &self,
        _params: Vec<Box<dyn Any + Send>>,
        _ctx: ProviderContext<'_>,
    ) -> Box<dyn Any + Send> {
        // Database is Clone (Arc-backed); cloning shares the same connection pool.
        Box::new(self.db.clone())
    }

    async fn on_application_shutdown(&self, _signal: Option<String>) {
        self.client.clone().shutdown().await;
    }
}
