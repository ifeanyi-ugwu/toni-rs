use std::sync::Arc;

use async_graphql::{ObjectType, Schema, SubscriptionType};
use async_trait::async_trait;
use toni::traits_helpers::{Injectable, ProviderFactory, ProviderRole};
use toni::{FxHashMap, GatewayTrait};

use crate::subscription_context_builder::SubscriptionContextBuilder;
use crate::subscription_gateway::GraphQLSubscriptionGateway;

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
    fn get_token(&self) -> String {
        format!("GraphQLSubscriptionGateway_{}", self.path)
    }

    async fn build(&self, _deps: FxHashMap<String, Injectable>) -> Injectable {
        let gateway = GraphQLSubscriptionGateway {
            schema: self.schema.clone(),
            context_builder: self.context_builder.clone(),
            path: self.path.clone(),
        };

        let role = ProviderRole::Gateway(Arc::new(
            Box::new(gateway.clone()) as Box<dyn GatewayTrait>
        ));
        let instance = Arc::new(Box::new(gateway) as Box<dyn toni::traits_helpers::Provider>);

        Injectable::new(instance, vec![role])
    }
}
