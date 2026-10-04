//! The juniper engine for `ulo-graphql` (transports DESIGN §7): [`Juniper<Q, M, Sub>`], bound under
//! the SPI key, `Juniper<Query, Mutation, Subscription> as dyn Engine`.
//!
//! juniper's `Context` is a type the schema's root types name, so the adapter resolves
//! `exec.get::<Q::Context>()` per call and hands it over as the context. The application's context
//! holds an `ExecutionRef` field, through which [`dep`] reads. `Engine::subscribe` is built on
//! `resolve_into_stream`.

use std::marker::PhantomData;

use juniper::{DefaultScalarValue, GraphQLSubscriptionType, GraphQLTypeAsync, RootNode};
use ulo::scope::Auto;
use ulo::{BoxFuture, Construct, ConstructError, Dep, Dependencies, ExecutionRef, LookupError, Resolver};
use ulo_graphql::{BoxStream, Engine, GqlRequest, GqlResponse};

/// The engine over the root node `RootNode<'static, Q, M, Sub>`, read as a `Dep`.
pub struct Juniper<Q, M, Sub> {
    pub(crate) _schema: PhantomData<fn() -> (Q, M, Sub)>,
}

impl<Q, M, Sub> Construct for Juniper<Q, M, Sub>
where
    Q: GraphQLTypeAsync<DefaultScalarValue> + Send + Sync + 'static,
    Q::TypeInfo: Send + Sync,
    Q::Context: Send + Sync + 'static,
    M: GraphQLTypeAsync<DefaultScalarValue, Context = Q::Context> + Send + Sync + 'static,
    M::TypeInfo: Send + Sync,
    Sub: GraphQLSubscriptionType<DefaultScalarValue, Context = Q::Context> + Send + Sync + 'static,
    Sub::TypeInfo: Send + Sync,
{
    type Scope = Auto;

    fn dependencies(d: &mut Dependencies) {
        d.add::<Dep<RootNode<'static, Q, M, Sub>>>();
    }

    async fn construct(r: &Resolver<'_>) -> Result<Self, ConstructError> {
        let _ = r;
        todo!()
    }
}

impl<Q, M, Sub> Engine for Juniper<Q, M, Sub>
where
    Q: GraphQLTypeAsync<DefaultScalarValue> + Send + Sync + 'static,
    Q::TypeInfo: Send + Sync,
    Q::Context: Send + Sync + 'static,
    M: GraphQLTypeAsync<DefaultScalarValue, Context = Q::Context> + Send + Sync + 'static,
    M::TypeInfo: Send + Sync,
    Sub: GraphQLSubscriptionType<DefaultScalarValue, Context = Q::Context> + Send + Sync + 'static,
    Sub::TypeInfo: Send + Sync,
{
    fn execute(&self, req: GqlRequest, exec: ExecutionRef) -> BoxFuture<'static, GqlResponse> {
        let _ = (req, exec);
        todo!()
    }

    fn subscribe(&self, req: GqlRequest, exec: ExecutionRef) -> BoxStream<'static, GqlResponse> {
        let _ = (req, exec);
        todo!()
    }

    fn sdl(&self) -> String {
        todo!()
    }

    /// juniper's `graphiql_source`.
    fn playground_html(&self, endpoint: &str, subscriptions: Option<&str>) -> Option<String> {
        let _ = (endpoint, subscriptions);
        todo!()
    }
}

/// `T` from the call's execution, through the context's `ExecutionRef`.
pub async fn dep<T: ?Sized + Send + Sync + 'static>(ctx: &impl AsRef<ExecutionRef>) -> Result<Dep<T>, LookupError> {
    let _ = ctx;
    todo!()
}
