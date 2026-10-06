//! The async-graphql engine for `ulo-graphql` (transports DESIGN §7): [`AsyncGraphql<S, C>`], bound
//! under the SPI key, `AsyncGraphql<ApiSchema, GqlContext> as dyn Engine`.
//!
//! The context type is a parameter of the adapter, since async-graphql takes context as
//! `Request::data(D)` for any `D` and no adapter can know the application's by convention. Per
//! call the adapter resolves `C` in the call's execution and inserts it as `Dep<C>` with
//! `Request::data`, and inserts the `ExecutionRef` itself as a second `data`, which [`dep`] reads.
//! A resolver reads the context with `ctx.data::<Dep<GqlContext>>()`.

use std::marker::PhantomData;
use std::sync::Arc;

use async_graphql::http::GraphiQLSource;
use async_graphql::{ObjectType, Schema, ServerError, SubscriptionType, Variables};
use futures_util::{StreamExt, future, stream};
use serde_json::{Map, Value};
use ulo::scope::Auto;
use ulo::{BoxFuture, Construct, ConstructError, Dep, Dependencies, ExecutionRef, ModuleRef, Resolver};
use ulo_graphql::{BoxStream, Engine, GqlError, GqlRequest, GqlResponse};

/// The engine over schema `S`, read as `Dep<S>`, with context type `C`, resolved per call from the
/// execution: `C` is an execution-scoped `#[injectable]` reading `Option<Ext<CurrentUser>>`,
/// `Dep<Loaders>` and `Option<Dep<RequestHead>>`, the last `Some` over HTTP alone.
///
/// `C` is resolved with the visibility of the module that binds the engine, inside the call's
/// execution, so it need not be exported to the module serving the endpoint. Its binding is read
/// at each call, not checked when the application wires: a context that is not bound fails each
/// call with a request error and an `error` log line.
pub struct AsyncGraphql<S, C> {
    pub(crate) schema: Dep<S>,
    pub(crate) _context: PhantomData<fn() -> C>,
    module: ModuleRef,
}

impl<S: Send + Sync + 'static, C: Send + Sync + 'static> Construct for AsyncGraphql<S, C> {
    type Scope = Auto;

    fn dependencies(d: &mut Dependencies) {
        d.add::<Dep<S>>().add::<ModuleRef>();
    }

    async fn construct(r: &Resolver<'_>) -> Result<Self, ConstructError> {
        Ok(AsyncGraphql { schema: r.dep::<S>().await?, _context: PhantomData, module: r.module() })
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
        let schema = (*self.schema).clone();
        let module = self.module.clone();
        Box::pin(async move {
            match request::<C>(&module, req, exec).await {
                Ok(request) => response_of(schema.execute(request).await),
                Err(response) => response,
            }
        })
    }

    fn subscribe(&self, req: GqlRequest, exec: ExecutionRef) -> BoxStream<'static, GqlResponse> {
        let schema = (*self.schema).clone();
        let module = self.module.clone();
        stream::once(async move { request::<C>(&module, req, exec).await })
            .flat_map(move |prepared| match prepared {
                // `execute_stream` borrows the schema for its stream; the session-data form owns a
                // clone, which a stream outliving this closure needs.
                Ok(request) => schema.execute_stream_with_session_data(request, Default::default()).map(response_of).boxed(),
                Err(response) => stream::once(future::ready(response)).boxed(),
            })
            .boxed()
    }

    fn sdl(&self) -> String {
        self.schema.sdl()
    }

    /// async-graphql's `GraphiQLSource`.
    fn playground_html(&self, endpoint: &str, subscriptions: Option<&str>) -> Option<String> {
        let source = GraphiQLSource::build().endpoint(endpoint);
        let source = match subscriptions {
            Some(subscriptions) => source.subscription_endpoint(subscriptions),
            None => source,
        };
        Some(source.finish())
    }
}

/// `T` from the call's execution, inside a resolver: `ctx.data::<ExecutionRef>()?.get::<T>()`.
///
/// It resolves with the execution's visibility, which is that of the module serving the endpoint
/// or the gateway; a binding only the engine's module sees is read through a field of the context
/// instead. A failed lookup is logged and answers the resolver an error whose message is withheld.
pub async fn dep<T: ?Sized + Send + Sync + 'static>(ctx: &async_graphql::Context<'_>) -> async_graphql::Result<Dep<T>> {
    let exec = ctx.data::<ExecutionRef>()?;
    exec.get::<T>().await.map_err(|error| {
        tracing::error!(%error, "a resolver's dependency could not be resolved");
        let mut withheld = async_graphql::Error::new("internal error");
        withheld.source = Some(Arc::new(error));
        withheld
    })
}

/// The async-graphql request for `req`, carrying the call's context and its execution. A context
/// that cannot be built is answered `Failed`: nothing executed, and the fault is the server's.
async fn request<C: Send + Sync + 'static>(
    module: &ModuleRef,
    req: GqlRequest,
    exec: ExecutionRef,
) -> Result<async_graphql::Request, GqlResponse> {
    let context = match module.with_execution(&exec).get::<C>().await {
        Ok(context) => context,
        Err(error) => {
            tracing::error!(%error, "the GraphQL context could not be built");
            return Err(GqlResponse::failed(vec![GqlError::new("the request's context could not be built")]));
        }
    };
    let variables = Variables::from_json(Value::Object(req.variables.unwrap_or_default()));
    let mut request = async_graphql::Request::new(req.query).variables(variables);
    if let Some(name) = req.operation_name {
        request = request.operation_name(name);
    }
    for (name, value) in req.extensions.unwrap_or_default() {
        match async_graphql::Value::from_json(value) {
            Ok(value) => {
                request.extensions.0.insert(name, value);
            }
            Err(_) => {
                return Err(GqlResponse::request_error(vec![GqlError::new(format!(
                    "the extension `{name}` is not a GraphQL value"
                ))]));
            }
        }
    }
    Ok(request.data(context).data(exec))
}

/// async-graphql's response as a `GqlResponse`.
///
/// async-graphql answers a request that failed before execution as a response whose `data` is
/// `null` and whose errors carry no `path`, a field error always naming its field, so that is how
/// a request error is told from an executed request whose data is `null`.
fn response_of(response: async_graphql::Response) -> GqlResponse {
    let errors: Vec<GqlError> = response.errors.iter().map(error_of).collect();
    let before_execution =
        response.data == async_graphql::Value::Null && !response.errors.is_empty() && response.errors.iter().all(|e| e.path.is_empty());
    let extensions: Map<String, Value> = response
        .extensions
        .into_iter()
        .filter_map(|(name, value)| value.into_json().ok().map(|value| (name, value)))
        .collect();
    let mut out = if before_execution {
        GqlResponse::request_error(errors)
    } else {
        GqlResponse::executed(response.data.into_json().unwrap_or(Value::Null), errors)
    };
    if !extensions.is_empty() {
        out = out.with_extensions(Value::Object(extensions));
    }
    out
}

/// One error as the specification's response format writes it: async-graphql serializes its
/// `ServerError` in that format already.
fn error_of(error: &ServerError) -> GqlError {
    serde_json::to_value(error)
        .and_then(serde_json::from_value::<GqlError>)
        .unwrap_or_else(|_| GqlError::new(error.message.clone()))
}
