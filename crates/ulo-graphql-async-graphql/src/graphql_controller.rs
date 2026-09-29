use crate::context_builder::ContextBuilder;
use crate::graphql_service::GraphQLService;
use async_graphql::{ObjectType, SubscriptionType};
use async_trait::async_trait;
use serde::Deserialize;
use std::sync::Arc;
use ulo::FxHashMap;
use ulo::dispatch::{Controller, ControllerFactory, Targets};
use ulo::http::Route;
use ulo::http::{Body, HttpMethod, HttpRequest, HttpResponse};
use ulo::spi::{BuildResult, Provider};
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
pub struct GraphQLControllerFactory<Query, Mutation, Subscription, Ctx>
where
    Query: ObjectType + 'static,
    Mutation: ObjectType + 'static,
    Subscription: SubscriptionType + 'static,
    Ctx: ContextBuilder,
{
    path: String,
    playground_enabled: bool,
    _phantom: std::marker::PhantomData<(Query, Mutation, Subscription, Ctx)>,
}

impl<Query, Mutation, Subscription, Ctx>
    GraphQLControllerFactory<Query, Mutation, Subscription, Ctx>
where
    Query: ObjectType + 'static,
    Mutation: ObjectType + 'static,
    Subscription: SubscriptionType + 'static,
    Ctx: ContextBuilder,
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
impl<Query, Mutation, Subscription, Ctx> ControllerFactory
    for GraphQLControllerFactory<Query, Mutation, Subscription, Ctx>
where
    Query: ObjectType + 'static,
    Mutation: ObjectType + 'static,
    Subscription: SubscriptionType + 'static,
    Ctx: ContextBuilder,
{
    fn token(&self) -> String {
        format!("GraphQLController_{}", self.path)
    }

    fn dependency_tokens(&self) -> Vec<String> {
        vec![ulo::di::token_of::<
            crate::GraphQLService<Query, Mutation, Subscription, Ctx>,
        >()]
    }

    async fn build(
        &self,
        dependencies: FxHashMap<String, Arc<Box<dyn Provider>>>,
    ) -> BuildResult<Arc<dyn Controller>> {
        let graphql_service = dependencies
            .get(&ulo::di::token_of::<
                crate::GraphQLService<Query, Mutation, Subscription, Ctx>,
            >())
            .expect("GraphQLService not found in dependencies")
            .clone();

        let mut routes: Vec<Arc<dyn Route>> = Vec::new();

        routes.push(Arc::new(GraphQLPostController::<
            Query,
            Mutation,
            Subscription,
            Ctx,
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

        Ok(Arc::new(GraphQLController {
            token: format!("GraphQLController_{}", self.path),
            routes,
        }))
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
struct GraphQLPostController<Query, Mutation, Subscription, Ctx>
where
    Query: ObjectType + 'static,
    Mutation: ObjectType + 'static,
    Subscription: SubscriptionType + 'static,
    Ctx: ContextBuilder,
{
    path: String,
    graphql_service: Arc<Box<dyn Provider>>,
    _phantom: std::marker::PhantomData<(Query, Mutation, Subscription, Ctx)>,
}

#[async_trait]
impl<Query, Mutation, Subscription, Ctx> Route
    for GraphQLPostController<Query, Mutation, Subscription, Ctx>
where
    Query: ObjectType + 'static,
    Mutation: ObjectType + 'static,
    Subscription: SubscriptionType + 'static,
    Ctx: ContextBuilder,
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

impl<Query, Mutation, Subscription, Ctx> GraphQLPostController<Query, Mutation, Subscription, Ctx>
where
    Query: ObjectType + 'static,
    Mutation: ObjectType + 'static,
    Subscription: SubscriptionType + 'static,
    Ctx: ContextBuilder,
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
            .await
            .expect("GraphQLService is a singleton, which resolves in any execution");

        let service = service_any
            .downcast_ref::<GraphQLService<Query, Mutation, Subscription, Ctx>>()
            .expect("Failed to downcast to GraphQLService");

        let response = service
            .execute(
                gql_request.query,
                gql_request.operation_name,
                gql_request.variables,
                &parts,
            )
            .await;

        // Convert to JSON response
        let response_json = serde_json::to_value(&response).unwrap();

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
