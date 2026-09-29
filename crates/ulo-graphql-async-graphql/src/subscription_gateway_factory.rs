use std::sync::Arc;

use crate::subscription_context_builder::SubscriptionContextBuilder;
use crate::subscription_gateway::GraphQLSubscriptionGateway;
use async_graphql::{ObjectType, Schema, SubscriptionType};
use async_trait::async_trait;
use ulo::FxHashMap;
use ulo::spi::{BuildResult, ProviderFactory, ProviderRole, Registration};
use ulo::ws::Gateway;

pub struct GraphQLSubscriptionGatewayFactory<Q, M, S>
where
    Q: ObjectType + 'static,
    M: ObjectType + 'static,
    S: SubscriptionType + 'static,
{
    schema: Arc<Schema<Q, M, S>>,
    context_builder: Arc<dyn SubscriptionContextBuilder>,
    path: String,
}

impl<Q, M, S> GraphQLSubscriptionGatewayFactory<Q, M, S>
where
    Q: ObjectType + 'static,
    M: ObjectType + 'static,
    S: SubscriptionType + 'static,
{
    pub fn new(
        schema: Arc<Schema<Q, M, S>>,
        context_builder: Arc<dyn SubscriptionContextBuilder>,
        path: String,
    ) -> Self {
        Self {
            schema,
            context_builder,
            path,
        }
    }
}

#[async_trait]
impl<Q, M, S> ProviderFactory for GraphQLSubscriptionGatewayFactory<Q, M, S>
where
    Q: ObjectType + 'static,
    M: ObjectType + 'static,
    S: SubscriptionType + 'static,
{
    fn token(&self) -> String {
        format!("GraphQLSubscriptionGateway_{}", self.path)
    }

    async fn build(&self, _deps: FxHashMap<String, Registration>) -> BuildResult<Registration> {
        let gateway = GraphQLSubscriptionGateway {
            schema: self.schema.clone(),
            context_builder: self.context_builder.clone(),
            path: self.path.clone(),
            init_payloads: std::sync::Arc::new(std::sync::Mutex::new(
                std::collections::HashMap::new(),
            )),
            abort_handles: std::sync::Arc::new(std::sync::Mutex::new(
                std::collections::HashMap::new(),
            )),
        };

        let role = ProviderRole::Gateway(Arc::new(gateway.clone()) as Arc<dyn Gateway>);
        let instance = Arc::new(gateway) as Arc<dyn ulo::spi::Provider>;

        Ok(Registration::new(instance, vec![role]))
    }
}
