//! The async-graphql engine for `ulo-graphql` (transports DESIGN §7): [`AsyncGraphql<S, C>`], bound
//! under the SPI key, `AsyncGraphql<ApiSchema, GqlContext> as dyn Engine`.
//!
//! The context type is a parameter of the adapter, since async-graphql takes context as
//! `Request::data(D)` for any `D` and no adapter can know the application's by convention. Per
//! call the adapter resolves `exec.get::<C>()` and inserts it with `Request::data`, and inserts the
//! `ExecutionRef` itself as a second `data`, which [`dep`] reads.

use std::marker::PhantomData;

use async_graphql::{ObjectType, Schema, SubscriptionType};
use ulo::scope::Auto;
use ulo::{BoxFuture, Construct, ConstructError, Dep, Dependencies, ExecutionRef, Resolver};
use ulo_graphql::{BoxStream, Engine, GqlRequest, GqlResponse};

/// The engine over schema `S`, read as `Dep<S>`, with context type `C`, resolved per call from the
/// execution: `C` is an execution-scoped `#[injectable]` reading `Option<Ext<CurrentUser>>`,
/// `Dep<Loaders>` and `Option<Dep<RequestHead>>`, the last `Some` over HTTP alone.
pub struct AsyncGraphql<S, C> {
    pub(crate) schema: Dep<S>,
    pub(crate) _context: PhantomData<fn() -> C>,
}

impl<S: Send + Sync + 'static, C: Send + Sync + 'static> Construct for AsyncGraphql<S, C> {
    type Scope = Auto;

    fn dependencies(d: &mut Dependencies) {
        d.add::<Dep<S>>();
    }

    async fn construct(r: &Resolver<'_>) -> Result<Self, ConstructError> {
        let _ = r;
        todo!()
    }
}

impl<Q, M, Sub, C> Engine for AsyncGraphql<Schema<Q, M, Sub>, C>
where
    Q: ObjectType + 'static,
    M: ObjectType + 'static,
    Sub: SubscriptionType + 'static,
    C: Send + Sync + 'static,
{
    fn execute(&self, req: GqlRequest, exec: ExecutionRef) -> BoxFuture<'static, GqlResponse> {
        let _ = (req, exec, &self.schema);
        todo!()
    }

    fn subscribe(&self, req: GqlRequest, exec: ExecutionRef) -> BoxStream<'static, GqlResponse> {
        let _ = (req, exec);
        todo!()
    }

    fn sdl(&self) -> String {
        todo!()
    }

    /// async-graphql's `GraphiQLSource`.
    fn playground_html(&self, endpoint: &str, subscriptions: Option<&str>) -> Option<String> {
        let _ = (endpoint, subscriptions);
        todo!()
    }
}

/// `T` from the call's execution, inside a resolver: `ctx.data::<ExecutionRef>()?.get::<T>()`.
pub async fn dep<T: ?Sized + Send + Sync + 'static>(ctx: &async_graphql::Context<'_>) -> async_graphql::Result<Dep<T>> {
    let _ = ctx;
    todo!()
}
