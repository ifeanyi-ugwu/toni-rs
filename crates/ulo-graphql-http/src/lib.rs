//! GraphQL over HTTP for `ulo` (transports DESIGN §7): the endpoint, a controller mounted
//! `.at(config.path)`, so HTTP guards, interceptors and pre-dispatch entries apply to it as to any
//! route; the GraphQL-over-HTTP status rules; the engine's playground; and, with
//! `GraphqlConfig::subscriptions`, the graphql-transport-ws gateway of `ulo-graphql-ws`.
//!
//! ```ignore
//! #[module(
//!     global,
//!     imports   = [
//!         ulo_ws::WsModule::for_root(),
//!         GraphqlModule::for_root(GraphqlConfig::at("/graphql").subscriptions("/graphql/ws")),
//!     ],
//!     providers = [
//!         with = |q: Dep<QueryRoot>, s: Dep<SubscriptionRoot>| Schema::new(q, EmptyMutation, s),
//!         AsyncGraphql<ApiSchema, GqlContext> as dyn Engine,
//!     ],
//!     exports   = [dyn Engine],
//! )]
//! pub struct ApiModule;
//! ```
//!
//! The engine is bound in a global module and exported, since `GraphqlModule`'s controllers read it
//! from their own module, which sees the globals' exports and not its importer's bindings.
//!
//! In `application/graphql-response+json` a request that executed answers 200, errors included,
//! and one that failed before execution 400; in the legacy `application/json` every response is
//! 200. The response media type is negotiated from `Accept`, and a request with no `Accept` reads
//! as accepting `application/graphql-response+json`. A POST whose body is not `application/json`
//! is refused 415, and a GET whose operation is a mutation 405.

mod controller;
mod module;
mod playground;

pub use module::{GraphqlConfig, GraphqlModule};
