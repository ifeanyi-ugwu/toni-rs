//! GraphQL over HTTP for `ulo` (transports DESIGN §7): the endpoint, a controller mounted
//! `.at(config.path)`, so HTTP guards, interceptors and pre-dispatch entries apply to it as to any
//! route; the GraphQL-over-HTTP status rules; the engine's playground; and, with
//! `GraphqlConfig::subscriptions`, the graphql-transport-ws gateway of `ulo-graphql-ws`.
//!
//! ```ignore
//! #[module(
//!     providers = [
//!         with = |q: Dep<QueryRoot>, s: Dep<SubscriptionRoot>| Schema::new(q, EmptyMutation, s),
//!         AsyncGraphql<ApiSchema, GqlContext> as dyn Engine,
//!     ],
//!     exports   = [dyn Engine],
//! )]
//! pub struct ApiSchemaModule;
//!
//! #[module(imports = [
//!     ulo_ws::WsModule::for_root(),
//!     GraphqlModule::for_root(GraphqlConfig::at("/graphql").subscriptions("/graphql/ws").engine_from(ApiSchemaModule)),
//! ])]
//! pub struct AppModule;
//! ```
//!
//! `GraphqlModule`'s controllers read the engine from their own module, which sees its imports'
//! exports and not its importer's bindings, so `engine_from` names the module binding the engine
//! and `GraphqlModule` imports it.
//!
//! In `application/graphql-response+json` a request that executed answers 200, errors included,
//! and one that failed before execution 400; in the legacy `application/json` every GraphQL
//! response is 200. A request the server could not run, its context failing to build, answers
//! 500 under both. The response media type is negotiated from `Accept`, and a request with no
//! `Accept` reads as accepting `application/graphql-response+json`. A POST whose body is not
//! `application/json` is refused 415, and a GET whose operation is a mutation 405 with `Allow`,
//! both through the error handlers as the router's refusals are.

mod controller;
mod module;
mod playground;

pub use module::{GraphqlConfig, GraphqlModule};
