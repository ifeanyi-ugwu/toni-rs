use crate::context_builder::ContextBuilder;
use crate::graphql_service::GraphQLService;
use async_trait::async_trait;
use juniper::{
    DefaultScalarValue, GraphQLSubscriptionType, GraphQLType, GraphQLTypeAsync, ScalarValue,
};
use serde::Deserialize;
use std::sync::Arc;
use ulo::FxHashMap;
use ulo::dispatch::{Controller, ControllerFactory, Targets};
use ulo::http::Route;
use ulo::http::{Body, HttpMethod, HttpRequest, HttpResponse};
use ulo::spi::Provider;
/// GraphQL request payload
#[derive(Debug, Deserialize)]
struct GraphQLRequest {
    query: String,
    #[serde(rename = "operationName")]
    operation_name: Option<String>,
    variables: Option<serde_json::Value>,
}

/// `ControllerFactory` for GraphQL endpoints.
///
/// This creates two endpoints:
/// - POST /graphql - Execute GraphQL queries
/// - GET /graphql - Serve GraphQL Playground (if enabled)
pub struct GraphQLControllerFactory<Query, Mutation, Subscription, Ctx, S = DefaultScalarValue>
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
    path: String,
    playground_enabled: bool,
    _phantom: std::marker::PhantomData<(Query, Mutation, Subscription, Ctx, S)>,
}

impl<Query, Mutation, Subscription, Ctx, S>
    GraphQLControllerFactory<Query, Mutation, Subscription, Ctx, S>
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
    pub fn new(path: String, playground_enabled: bool) -> Self {
        Self {
            path,
            playground_enabled,
            _phantom: std::marker::PhantomData,
        }
    }
}

#[async_trait]
impl<Query, Mutation, Subscription, Ctx, S> ControllerFactory
    for GraphQLControllerFactory<Query, Mutation, Subscription, Ctx, S>
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
        format!("GraphQLController_{}", self.path)
    }

    fn dependency_tokens(&self) -> Vec<String> {
        vec![ulo::di::token_of::<
            crate::GraphQLService<Query, Mutation, Subscription, Ctx, S>,
        >()]
    }

    async fn build(
        &self,
        dependencies: FxHashMap<String, Arc<Box<dyn Provider>>>,
    ) -> Arc<dyn Controller> {
        let graphql_service = dependencies
            .get(&ulo::di::token_of::<
                crate::GraphQLService<Query, Mutation, Subscription, Ctx, S>,
            >())
            .expect("GraphQLService not found in dependencies")
            .clone();

        let mut routes: Vec<Arc<dyn Route>> = Vec::new();

        routes.push(Arc::new(GraphQLPostController::<
            Query,
            Mutation,
            Subscription,
            Ctx,
            S,
        > {
            path: self.path.clone(),
            graphql_service,
            _phantom: std::marker::PhantomData,
        }));

        if self.playground_enabled {
            routes.push(Arc::new(GraphQLPlaygroundController {
                path: self.path.clone(),
                playground_html: include_str!("playground.html").to_string(),
            }));
        }

        Arc::new(GraphQLController {
            token: format!("GraphQLController_{}", self.path),
            routes,
        })
    }
}

/// The single GraphQL controller: the POST query endpoint and, optionally, the
/// GET playground endpoint.
struct GraphQLController {
    token: String,
    routes: Vec<Arc<dyn Route>>,
}

#[async_trait]
impl Controller for GraphQLController {
    fn token(&self) -> String {
        self.token.clone()
    }

    fn targets(&self) -> Targets {
        Targets::Http(self.routes.clone())
    }
}

/// POST controller for executing GraphQL queries
struct GraphQLPostController<Query, Mutation, Subscription, Ctx, S = DefaultScalarValue>
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
    path: String,
    graphql_service: Arc<Box<dyn Provider>>,
    _phantom: std::marker::PhantomData<(Query, Mutation, Subscription, Ctx, S)>,
}

#[async_trait]
impl<Query, Mutation, Subscription, Ctx, S> Route
    for GraphQLPostController<Query, Mutation, Subscription, Ctx, S>
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
    async fn execute(
        &self,
        ctx: &ulo::http::HttpContext,
    ) -> ulo::dispatch::ExecutionResult<HttpResponse, ulo::http::HttpError> {
        let Some(req) = ctx.take_request() else {
            return ulo::dispatch::ExecutionResult::Ok(HttpResponse {
                status: 400,
                headers: vec![],
                body: Some(Body::json(serde_json::json!({
                    "errors": [{"message": "request body was already read"}]
                }))),
            });
        };
        self.execute_inner(req, ctx).await.into()
    }

    fn path(&self) -> String {
        self.path.clone()
    }

    fn method(&self) -> HttpMethod {
        HttpMethod::POST
    }
}

impl<Query, Mutation, Subscription, Ctx, S>
    GraphQLPostController<Query, Mutation, Subscription, Ctx, S>
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
    async fn execute_inner(&self, req: HttpRequest, ctx: &ulo::http::HttpContext) -> HttpResponse {
        let (parts, body) = req.into_parts();
        let body_bytes = match body.collect().await {
            Ok(b) => b,
            Err(e) => {
                return HttpResponse {
                    status: 400,
                    body: Some(Body::json(serde_json::json!({
                        "errors": [{"message": format!("Failed to read request body: {}", e)}]
                    }))),
                    headers: vec![],
                };
            }
        };

        // Parse GraphQL request from body
        let gql_request: GraphQLRequest = match serde_json::from_slice(&body_bytes) {
            Ok(req) => req,
            Err(e) => {
                return HttpResponse {
                    status: 400,
                    body: Some(Body::json(serde_json::json!({
                        "errors": [{"message": format!("Invalid GraphQL request: {}", e)}]
                    }))),
                    headers: vec![],
                };
            }
        };

        let service_any = self
            .graphql_service
            .resolve(ulo::di::Execution::Http(ctx.clone()))
            .await;

        let service = service_any
            .downcast_ref::<GraphQLService<Query, Mutation, Subscription, Ctx, S>>()
            .expect("Failed to downcast to GraphQLService");

        let response_json = service
            .execute(
                gql_request.query,
                gql_request.operation_name,
                gql_request.variables,
                &parts,
            )
            .await;

        HttpResponse {
            status: 200,
            body: Some(Body::json(response_json)),
            headers: vec![],
        }
    }
}

/// GET controller for serving GraphQL Playground
struct GraphQLPlaygroundController {
    path: String,
    playground_html: String,
}

#[async_trait]
impl Route for GraphQLPlaygroundController {
    async fn execute(
        &self,
        _ctx: &ulo::http::HttpContext,
    ) -> ulo::dispatch::ExecutionResult<HttpResponse, ulo::http::HttpError> {
        HttpResponse {
            status: 200,
            body: Some(
                Body::text(self.playground_html.clone())
                    .with_content_type("text/html; charset=utf-8"),
            ),
            headers: vec![],
        }
        .into()
    }

    fn path(&self) -> String {
        self.path.clone()
    }

    fn method(&self) -> HttpMethod {
        HttpMethod::GET
    }
}
