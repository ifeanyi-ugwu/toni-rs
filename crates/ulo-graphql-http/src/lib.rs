//! GraphQL over HTTP for `ulo` (transports DESIGN §7): the endpoint, a controller mounted
//! `.at(config.path)`, so HTTP guards, interceptors and pre-dispatch entries apply to it as to any
//! route; the GraphQL-over-HTTP status rules; and the engine's playground.
//!
//! ```ignore
//! #[module(
//!     imports   = [GraphqlModule::for_root(GraphqlConfig::at("/graphql").subscriptions("/graphql/ws"))],
//!     providers = [
//!         with = |q: Dep<QueryRoot>, s: Dep<SubscriptionRoot>| Schema::new(q, EmptyMutation, s),
//!         AsyncGraphql<ApiSchema, GqlContext> as dyn Engine,
//!     ],
//! )]
//! pub struct ApiModule;
//! ```
//!
//! In `application/graphql-response+json` a request that executed answers 200, errors included,
//! and one that failed before execution 400; in the legacy `application/json` every response is
//! 200. The response media type is negotiated from `Accept`. A POST whose body is not
//! `application/json` is refused 415, and a GET whose operation is a mutation 405.

mod controller;
mod module;
mod playground;

pub use module::{GraphqlConfig, GraphqlModule};
