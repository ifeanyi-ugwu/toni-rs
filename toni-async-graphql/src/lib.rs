//! # toni-async-graphql
//!
//! async-graphql integration for the Toni framework.
//!
//! This crate provides seamless integration between [async-graphql](https://github.com/async-graphql/async-graphql)
//! and the [Toni](https://github.com/toni-rs/toni) web framework, enabling you to build
//! type-safe GraphQL APIs with dependency injection, middleware, guards, and all Toni features.
//!
//! ## Features
//!
//! - **Full async-graphql support** - Use all async-graphql features natively
//! - **Dependency Injection** - Inject Toni services into your context builders
//! - **User-controlled context** - Build GraphQL context however you want
//! - **Guards & Interceptors** - Use Toni's guards and interceptors with GraphQL
//! - **GraphQL Playground** - Built-in playground for development
//! - **Zero overhead** - Compiles to native async-graphql code
//!
//! ## Quick Start
//!
//! ```ignore
//! use toni::{module, ToniFactory};
//! use toni_axum::AxumAdapter;
//! use toni_async_graphql::{GraphQLModule, DefaultContextBuilder, async_graphql::*};
//!
//! struct Query;
//!
//! #[Object]
//! impl Query {
//!     async fn hello(&self) -> &str {
//!         "Hello, world!"
//!     }
//! }
//!
//! fn build_graphql_module()
//! -> GraphQLModule<Query, EmptyMutation, EmptySubscription, DefaultContextBuilder> {
//!     let schema = Schema::build(Query, EmptyMutation, EmptySubscription).finish();
//!     GraphQLModule::for_root(schema, DefaultContextBuilder)
//! }
//!
//! #[module(imports: [build_graphql_module()], controllers: [], providers: [], exports: [])]
//! impl AppModule {}
//!
//! #[tokio::main]
//! async fn main() {
//!     let mut app = ToniFactory::new()
//!         .create_with(AppModule)
//!         .await;
//!     app.use_http_adapter(AxumAdapter::new(), 3000, "127.0.0.1").unwrap();
//!     app.start().await.unwrap();
//! }
//! ```
//!
//! ## Custom Context
//!
//! Build GraphQL context with access to Toni's DI system:
//!
//! ```ignore
//! use toni_async_graphql::{ContextBuilder, async_graphql::Data};
//! use toni::{HttpRequest, injectable};
//! use async_trait::async_trait;
//!
//! #[injectable]
//! pub struct MyContextBuilder {
//!     #[inject]
//!     auth_service: AuthService,
//!     #[inject]
//!     db_pool: DatabasePool,
//! }
//!
//! #[async_trait]
//! impl ContextBuilder for MyContextBuilder {
//!     async fn build(&self, req: &HttpRequest) -> Data {
//!         let mut data = Data::default();
//!
//!         // Add HTTP request
//!         data.insert(req.clone());
//!
//!         // Add user from auth service (DI!)
//!         if let Some(user) = self.auth_service.verify_token(req) {
//!             data.insert(user);
//!         }
//!
//!         // Add database pool
//!         data.insert(self.db_pool.clone());
//!
//!         data
//!     }
//! }
//! ```
//!
//! ## Accessing Context in Resolvers
//!
//! ```ignore
//! use async_graphql::{Object, Context, Result};
//!
//! struct Query;
//!
//! #[Object]
//! impl Query {
//!     async fn me(&self, ctx: &Context<'_>) -> Result<User> {
//!         // Get user from context (added by auth middleware/guard)
//!         let user = ctx.data::<User>()?;
//!         Ok(user.clone())
//!     }
//!
//!     async fn user(&self, ctx: &Context<'_>, id: i32) -> Result<User> {
//!         // Get DI service from context
//!         let db_pool = ctx.data::<DatabasePool>()?;
//!         db_pool.find_user(id).await
//!     }
//! }
//! ```

mod context_builder;
mod graphql_controller;
mod graphql_module;
mod graphql_service;
mod graphql_service_factory;
mod subscription_context_builder;
mod subscription_gateway;
mod subscription_gateway_factory;

// Re-export key types
pub use context_builder::{ContextBuilder, DefaultContextBuilder};
pub use graphql_module::GraphQLModule;
pub use graphql_service::GraphQLService;
pub use subscription_context_builder::{
    DefaultSubscriptionContextBuilder, SubscriptionContextBuilder,
};

// Re-export async-graphql for convenience
pub use async_graphql;

/// Prelude module with common imports
pub mod prelude {
    pub use crate::{
        ContextBuilder, DefaultContextBuilder, DefaultSubscriptionContextBuilder, GraphQLModule,
        GraphQLService, SubscriptionContextBuilder,
    };
    pub use async_graphql::{
        Context, EmptyMutation, EmptySubscription, Enum, InputObject, Interface, Object, Schema,
        SimpleObject, Subscription, Union,
    };
}
