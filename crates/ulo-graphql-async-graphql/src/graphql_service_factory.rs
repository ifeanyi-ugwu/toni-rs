use crate::context_builder::ContextBuilder;
use crate::graphql_service::GraphQLService;
use async_graphql::{ObjectType, Schema, SubscriptionType};
use async_trait::async_trait;
use std::sync::Arc;
use ulo::FxHashMap;
use ulo::spi::{BuildResult, Injectable, Provider, ProviderFactory};
/// `ProviderFactory` for `GraphQLService` — registered during module scanning.
pub struct GraphQLServiceFactory<Query, Mutation, Subscription, Ctx>
where
    Query: ObjectType + 'static,
    Mutation: ObjectType + 'static,
    Subscription: SubscriptionType + 'static,
    Ctx: ContextBuilder,
{
    schema: Arc<Schema<Query, Mutation, Subscription>>,
    context_builder: Arc<Ctx>,
}

impl<Query, Mutation, Subscription, Ctx> GraphQLServiceFactory<Query, Mutation, Subscription, Ctx>
where
    Query: ObjectType + 'static,
    Mutation: ObjectType + 'static,
    Subscription: SubscriptionType + 'static,
    Ctx: ContextBuilder,
{
    pub fn new(
        schema: Arc<Schema<Query, Mutation, Subscription>>,
        context_builder: Arc<Ctx>,
    ) -> Self {
        Self {
            schema,
            context_builder,
        }
    }
}

#[async_trait]
impl<Query, Mutation, Subscription, Ctx> ProviderFactory
    for GraphQLServiceFactory<Query, Mutation, Subscription, Ctx>
where
    Query: ObjectType + 'static,
    Mutation: ObjectType + 'static,
    Subscription: SubscriptionType + 'static,
    Ctx: ContextBuilder,
{
    fn token(&self) -> String {
        ulo::di::token_of::<crate::GraphQLService<Query, Mutation, Subscription, Ctx>>()
    }

    async fn build(
        &self,
        _deps: FxHashMap<String, ulo::spi::Injectable>,
    ) -> BuildResult<Injectable> {
        Ok(Injectable::new(
            Arc::new(Box::new(GraphQLService::new(
                self.schema.clone(),
                self.context_builder.clone(),
            )) as Box<dyn Provider>),
            vec![],
        ))
    }
}
