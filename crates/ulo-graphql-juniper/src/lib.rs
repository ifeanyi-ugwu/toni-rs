//! The juniper engine for `ulo-graphql` (transports DESIGN §7): [`Juniper<Q, M, Sub>`], bound under
//! the SPI key, `Juniper<Query, Mutation, Subscription> as dyn Engine`.
//!
//! juniper's `Context` is a type the schema's root types name, so the adapter resolves
//! `Q::Context` in the call's execution and hands it over as the context. The application's
//! context holds an `ExecutionRef` field, through which [`dep`] reads. `Engine::subscribe` is
//! built on `resolve_into_stream`.

use std::any::Any;
use std::marker::PhantomData;
use std::sync::Arc;

use futures::channel::mpsc;
use futures::stream::{self, BoxStream as BorrowedStream};
use futures::{SinkExt, StreamExt, future};
use juniper::http::{GraphQLRequest, GraphQLResponse};
use juniper::{
    DefaultScalarValue, ExecutionError, GraphQLError, GraphQLSubscriptionType, GraphQLTypeAsync, InputValue, RootNode,
    Value as JuniperValue,
};
use serde_json::{Map, Value};
use ulo::scope::Auto;
use ulo::{BoxFuture, Construct, ConstructError, Dep, Dependencies, ExecutionRef, LookupError, ModuleRef, Resolver};
use ulo_graphql::{BoxStream, Engine, GqlError, GqlRequest, GqlResponse};

/// The root node the engine reads.
type Root<Q, M, Sub> = RootNode<'static, Q, M, Sub>;

/// The engine over the root node `RootNode<'static, Q, M, Sub>`, read as a `Dep`.
///
/// `Q::Context` is resolved with the visibility of the module that binds the engine, inside the
/// call's execution, so it need not be exported to the module serving the endpoint. Its binding is
/// read at each call, not checked when the application wires: a context that is not bound fails
/// each call with a request error and an `error` log line.
///
/// A subscription answers one response per event of its root field; a query or mutation sent to
/// `subscribe` answers one response.
pub struct Juniper<Q, M, Sub> {
    pub(crate) _schema: PhantomData<fn() -> (Q, M, Sub)>,
    /// The `Arc<RootNode<'static, Q, M, Sub>>`, erased: a field naming `RootNode<'static, Q, M, Sub>`
    /// would need juniper's bounds on the struct itself.
    root: Arc<dyn Any + Send + Sync>,
    module: ModuleRef,
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
        d.add::<Dep<Root<Q, M, Sub>>>().add::<ModuleRef>();
    }

    async fn construct(r: &Resolver<'_>) -> Result<Self, ConstructError> {
        let root: Arc<Root<Q, M, Sub>> = r.dep::<Root<Q, M, Sub>>().await?.into_arc();
        Ok(Juniper { _schema: PhantomData, root, module: r.module() })
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
        let root = self.root();
        let module = self.module.clone();
        Box::pin(async move {
            let Some(root) = root else { return unbuilt() };
            let context = match context::<Q::Context>(&module, &exec).await {
                Ok(context) => context,
                Err(response) => return response,
            };
            let request = match request_of(req) {
                Ok(request) => request,
                Err(response) => return response,
            };
            response_of(request.execute(&*root, &*context).await)
        })
    }

    fn subscribe(&self, req: GqlRequest, exec: ExecutionRef) -> BoxStream<'static, GqlResponse> {
        let root = self.root();
        let module = self.module.clone();
        // `resolve_into_stream`'s stream borrows the root node, the context and the request, so
        // one future owns all three and forwards each response through a channel; polling the
        // channel beside that future drives it, with no task spawned.
        let (sender, receiver) = mpsc::channel::<GqlResponse>(0);
        let driver = drive::<Q, M, Sub>(root, module, req, exec, sender);
        stream::select(receiver, stream::once(driver).filter_map(|()| future::ready(None::<GqlResponse>))).boxed()
    }

    fn sdl(&self) -> String {
        self.root().map(|root| root.as_sdl()).unwrap_or_default()
    }

    /// juniper's `graphiql_source`.
    fn playground_html(&self, endpoint: &str, subscriptions: Option<&str>) -> Option<String> {
        Some(juniper::http::graphiql::graphiql_source(endpoint, subscriptions))
    }
}

impl<Q, M, Sub> Juniper<Q, M, Sub>
where
    Q: GraphQLTypeAsync<DefaultScalarValue> + Send + Sync + 'static,
    Q::TypeInfo: Send + Sync,
    Q::Context: Send + Sync + 'static,
    M: GraphQLTypeAsync<DefaultScalarValue, Context = Q::Context> + Send + Sync + 'static,
    M::TypeInfo: Send + Sync,
    Sub: GraphQLSubscriptionType<DefaultScalarValue, Context = Q::Context> + Send + Sync + 'static,
    Sub::TypeInfo: Send + Sync,
{
    /// The root node `construct` stored. `None` is unreachable, `construct` storing nothing else.
    fn root(&self) -> Option<Arc<Root<Q, M, Sub>>> {
        Arc::clone(&self.root).downcast::<Root<Q, M, Sub>>().ok()
    }
}

/// `T` from the call's execution, through the context's `ExecutionRef`.
///
/// It resolves with the execution's visibility, which is that of the module serving the endpoint
/// or the gateway; a binding only the engine's module sees is read through a field of the context
/// instead, which is resolved in the context's own module.
pub async fn dep<T: ?Sized + Send + Sync + 'static>(ctx: &impl AsRef<ExecutionRef>) -> Result<Dep<T>, LookupError> {
    ctx.as_ref().get::<T>().await
}

/// One subscription: the context, then `resolve_into_stream`, each event forwarded to `sender`
/// until the client stops reading.
async fn drive<Q, M, Sub>(
    root: Option<Arc<Root<Q, M, Sub>>>,
    module: ModuleRef,
    req: GqlRequest,
    exec: ExecutionRef,
    mut sender: mpsc::Sender<GqlResponse>,
) where
    Q: GraphQLTypeAsync<DefaultScalarValue> + Send + Sync + 'static,
    Q::TypeInfo: Send + Sync,
    Q::Context: Send + Sync + 'static,
    M: GraphQLTypeAsync<DefaultScalarValue, Context = Q::Context> + Send + Sync + 'static,
    M::TypeInfo: Send + Sync,
    Sub: GraphQLSubscriptionType<DefaultScalarValue, Context = Q::Context> + Send + Sync + 'static,
    Sub::TypeInfo: Send + Sync,
{
    let Some(root) = root else {
        let _ = sender.send(unbuilt()).await;
        return;
    };
    let context = match context::<Q::Context>(&module, &exec).await {
        Ok(context) => context,
        Err(response) => {
            let _ = sender.send(response).await;
            return;
        }
    };
    let request = match request_of(req) {
        Ok(request) => request,
        Err(response) => {
            let _ = sender.send(response).await;
            return;
        }
    };
    let resolved = juniper::http::resolve_into_stream(&request, &*root, &*context).await;
    let mut events = match resolved {
        Err(GraphQLError::NotSubscription) => {
            let response = response_of(request.execute(&*root, &*context).await);
            let _ = sender.send(response).await;
            return;
        }
        Err(error) => {
            let _ = sender.send(response_of(GraphQLResponse::from_result(Err(error)))).await;
            return;
        }
        Ok((_, errors)) if !errors.is_empty() => {
            let errors = errors.iter().map(error_of).collect();
            let _ = sender.send(GqlResponse::executed(Value::Null, errors)).await;
            return;
        }
        Ok((JuniperValue::Object(fields), _)) => {
            let streams: Vec<BorrowedStream<'_, GqlResponse>> = fields
                .into_iter()
                .map(|(name, value)| match value {
                    JuniperValue::Scalar(values) => values.map(move |event| event_response(&name, event)).boxed(),
                    _ => stream::once(future::ready(GqlResponse::executed(field(&name, Value::Null), Vec::new()))).boxed(),
                })
                .collect();
            stream::select_all(streams)
        }
        Ok(_) => {
            let _ = sender.send(GqlResponse::executed(Value::Null, Vec::new())).await;
            return;
        }
    };
    while let Some(response) = events.next().await {
        if sender.send(response).await.is_err() {
            return;
        }
    }
}

/// The context, resolved in `exec` with the engine module's visibility. A context that cannot be
/// built is answered `Failed`: nothing executed, and the fault is the server's.
async fn context<C: Send + Sync + 'static>(module: &ModuleRef, exec: &ExecutionRef) -> Result<Dep<C>, GqlResponse> {
    module.with_execution(exec).get::<C>().await.map_err(|error| {
        tracing::error!(%error, "the GraphQL context could not be built");
        GqlResponse::failed(vec![GqlError::new("the request's context could not be built")])
    })
}

/// juniper's request for `req`. juniper reads no request extensions, and they are dropped.
fn request_of(req: GqlRequest) -> Result<GraphQLRequest<DefaultScalarValue>, GqlResponse> {
    let variables = match req.variables {
        Some(variables) => match serde_json::from_value::<InputValue<DefaultScalarValue>>(Value::Object(variables)) {
            Ok(variables) => Some(variables),
            Err(error) => {
                return Err(GqlResponse::request_error(vec![GqlError::new(format!("`variables` is not a GraphQL value: {error}"))]));
            }
        },
        None => None,
    };
    Ok(GraphQLRequest::new(req.query, req.operation_name, variables))
}

/// juniper's response as a `GqlResponse`: an `Err` inside it is a request that failed before
/// execution.
fn response_of(response: GraphQLResponse<DefaultScalarValue>) -> GqlResponse {
    let executed = response.is_ok();
    let mut json = serde_json::to_value(&response).unwrap_or(Value::Null);
    let errors: Vec<GqlError> = json
        .get_mut("errors")
        .map(Value::take)
        .and_then(|errors| serde_json::from_value(errors).ok())
        .unwrap_or_default();
    if executed {
        let data = json.get_mut("data").map(Value::take).unwrap_or(Value::Null);
        GqlResponse::executed(data, errors)
    } else if errors.is_empty() {
        GqlResponse::request_error(vec![GqlError::new("the request could not be executed")])
    } else {
        GqlResponse::request_error(errors)
    }
}

/// One subscription event of the root field `name`.
fn event_response(name: &str, event: Result<JuniperValue<DefaultScalarValue>, ExecutionError<DefaultScalarValue>>) -> GqlResponse {
    match event {
        Ok(value) => GqlResponse::executed(field(name, serde_json::to_value(&value).unwrap_or(Value::Null)), Vec::new()),
        Err(error) => GqlResponse::executed(field(name, Value::Null), vec![error_of(&error)]),
    }
}

/// `{name: value}`.
fn field(name: &str, value: Value) -> Value {
    let mut data = Map::new();
    data.insert(name.to_owned(), value);
    Value::Object(data)
}

/// One error as the specification's response format writes it, which juniper's serialization of
/// `ExecutionError` follows.
fn error_of(error: &ExecutionError<DefaultScalarValue>) -> GqlError {
    serde_json::to_value(error)
        .and_then(serde_json::from_value::<GqlError>)
        .unwrap_or_else(|_| GqlError::new(error.error().message()))
}

/// The answer for a root node `construct` did not store, which cannot happen.
fn unbuilt() -> GqlResponse {
    GqlResponse::failed(vec![GqlError::new("the GraphQL engine is not built")])
}
