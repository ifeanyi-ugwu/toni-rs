use crate::context_builder::ContextBuilder;
use crate::graphql_service::GraphQLService;
use async_trait::async_trait;
use juniper::{
    DefaultScalarValue, GraphQLSubscriptionType, GraphQLType, GraphQLTypeAsync, RootNode,
    ScalarValue,
};
use std::sync::Arc;
use ulo::FxHashMap;
use ulo::spi::{BuildResult, Provider, ProviderFactory, Registration};
/// `ProviderFactory` for `GraphQLService` — registered during module scanning.
pub struct GraphQLServiceFactory<Query, Mutation, Subscription, Ctx, S = DefaultScalarValue>
where
    Query: GraphQLType<S, Context = Ctx::Context>
        + GraphQLTypeAsync<S, Context = Ctx::Context>
        + Send
        + Sync
        + 'static,
    Mutation: GraphQLType<S, Context = Ctx::Context>
        + GraphQLTypeAsync<S, Context = Ctx::Context>
        + Send
        + Sync
        + 'static,
    Subscription: GraphQLType<S, Context = Ctx::Context>
        + GraphQLSubscriptionType<S, Context = Ctx::Context>
        + Send
        + Sync
        + 'static,
    Ctx: ContextBuilder,
    Ctx::Context: Send + Sync,
    S: ScalarValue + Send + Sync + 'static,
    Query::TypeInfo: Send + Sync,
    Mutation::TypeInfo: Send + Sync,
    Subscription::TypeInfo: Send + Sync,
{
    schema: Arc<RootNode<'static, Query, Mutation, Subscription, S>>,
    context_builder: Arc<Ctx>,
}

impl<Query, Mutation, Subscription, Ctx, S>
    GraphQLServiceFactory<Query, Mutation, Subscription, Ctx, S>
where
    Query: GraphQLType<S, Context = Ctx::Context>
        + GraphQLTypeAsync<S, Context = Ctx::Context>
        + Send
        + Sync
        + 'static,
    Mutation: GraphQLType<S, Context = Ctx::Context>
        + GraphQLTypeAsync<S, Context = Ctx::Context>
        + Send
        + Sync
        + 'static,
    Subscription: GraphQLType<S, Context = Ctx::Context>
        + GraphQLSubscriptionType<S, Context = Ctx::Context>
        + Send
        + Sync
        + 'static,
    Ctx: ContextBuilder,
    Ctx::Context: Send + Sync,
    S: ScalarValue + Send + Sync + 'static,
    Query::TypeInfo: Send + Sync,
    Mutation::TypeInfo: Send + Sync,
    Subscription::TypeInfo: Send + Sync,
{
    pub fn new(
        schema: Arc<RootNode<'static, Query, Mutation, Subscription, S>>,
        context_builder: Arc<Ctx>,
    ) -> Self {
        Self {
            schema,
            context_builder,
        }
    }
}

#[async_trait]
impl<Query, Mutation, Subscription, Ctx, S> ProviderFactory
    for GraphQLServiceFactory<Query, Mutation, Subscription, Ctx, S>
where
    Query: GraphQLType<S, Context = Ctx::Context>
        + GraphQLTypeAsync<S, Context = Ctx::Context>
        + Send
        + Sync
        + 'static,
    Mutation: GraphQLType<S, Context = Ctx::Context>
        + GraphQLTypeAsync<S, Context = Ctx::Context>
        + Send
        + Sync
        + 'static,
    Subscription: GraphQLType<S, Context = Ctx::Context>
        + GraphQLSubscriptionType<S, Context = Ctx::Context>
        + Send
        + Sync
        + 'static,
    Ctx: ContextBuilder,
    Ctx::Context: Send + Sync,
    S: ScalarValue + Send + Sync + 'static,
    Query::TypeInfo: Send + Sync,
    Mutation::TypeInfo: Send + Sync,
    Subscription::TypeInfo: Send + Sync,
{
    fn token(&self) -> String {
        ulo::di::token_of::<crate::GraphQLService<Query, Mutation, Subscription, Ctx, S>>()
    }

    async fn build(
        &self,
        _deps: FxHashMap<String, ulo::spi::Registration>,
    ) -> BuildResult<Registration> {
        Ok(Registration::new(
            Arc::new(GraphQLService::new(
                self.schema.clone(),
                self.context_builder.clone(),
            )) as Arc<dyn Provider>,
            vec![],
        ))
    }
}
