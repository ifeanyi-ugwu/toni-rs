//! `GraphqlModule` and its configuration.

use std::borrow::Cow;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;
use std::sync::Arc;

use ulo::{Bound, Module, ModuleDef, ModuleIdentity};
use ulo_graphql_ws::GraphqlWs;
use ulo_graphql_ws::__private::InitTimeout;
use ulo_transport::prepare::{Failures, zero_bound};

use crate::controller::{EndpointSettings, GraphqlEndpoint};

/// Serves one engine at one path: the engine bound under `dyn Engine`, or `dyn Engine @ Q` once
/// [`GraphqlConfig::engine`] names a qualifier, so a second `GraphqlModule` at another path serves
/// a second schema with no GraphQL rule added.
///
/// The module registers the endpoint, and with [`GraphqlConfig::subscriptions`] the
/// graphql-transport-ws gateway, as its own controllers, so the engine binding has to be visible
/// from it. [`GraphqlConfig::engine_from`] names the module that binds and exports the engine, and
/// this module imports it; the module that imports this one is not visible from it. The gateway
/// also needs `WsModule` imported once by the application.
///
/// ```ignore
/// #[module(
///     providers = [
///         with = |q: Dep<QueryRoot>, s: Dep<SubscriptionRoot>| Schema::new(q, EmptyMutation, s),
///         AsyncGraphql<ApiSchema, GqlContext> as dyn Engine,
///     ],
///     exports   = [dyn Engine],
/// )]
/// pub struct ApiSchemaModule;
///
/// #[module(imports = [
///     WsModule::for_root(),
///     GraphqlModule::for_root(GraphqlConfig::at("/graphql").subscriptions("/graphql/ws").engine_from(ApiSchemaModule)),
/// ])]
/// pub struct AppModule;
/// ```
pub struct GraphqlModule<Q = ()> {
    pub(crate) config: GraphqlConfig<Q>,
}

impl<Q: Send + Sync + 'static> GraphqlModule<Q> {
    pub fn for_root(config: GraphqlConfig<Q>) -> Self {
        GraphqlModule { config }
    }
}

/// The configuration is the identity: the same configuration imported twice is one module, and
/// two paths are two.
impl<Q: Send + Sync + 'static> Module for GraphqlModule<Q> {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_value(self)
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        let config = &self.config;
        let path = normalize(&config.path);
        let subscriptions = config.subscriptions.as_deref().map(normalize);
        m.value(EndpointSettings::<Q> {
            path: path.clone(),
            subscriptions: subscriptions.clone(),
            playground: config.playground,
            _engine: PhantomData,
        });
        if let Some(engine) = &config.engine_module {
            m.import(engine.clone());
        }
        m.controller::<GraphqlEndpoint<Q>>().at(path);
        if let Some(subscriptions) = subscriptions {
            let mut failures = Failures::new();
            failures.extend(zero_bound(
                "connection_init_timeout",
                config.connection_init_timeout,
                "close every connection before its `connection_init` could arrive",
            ));
            m.try_value(failures.into_result().map(|()| InitTimeout(config.connection_init_timeout)));
            m.controller::<GraphqlWs<Q>>().at(subscriptions);
        }
    }
}

impl<Q> Clone for GraphqlModule<Q> {
    fn clone(&self) -> Self {
        GraphqlModule { config: self.config.clone() }
    }
}

impl<Q> PartialEq for GraphqlModule<Q> {
    fn eq(&self, other: &Self) -> bool {
        self.config == other.config
    }
}

impl<Q> Eq for GraphqlModule<Q> {}

impl<Q> Hash for GraphqlModule<Q> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.config.hash(state);
    }
}

/// Where the endpoint is served and what it offers.
pub struct GraphqlConfig<Q = ()> {
    pub(crate) path: Cow<'static, str>,
    pub(crate) subscriptions: Option<Cow<'static, str>>,
    pub(crate) playground: bool,
    pub(crate) connection_init_timeout: Bound,
    pub(crate) engine_module: Option<EngineModule>,
    pub(crate) _engine: PhantomData<fn() -> Q>,
}

/// The module [`GraphqlConfig::engine_from`] named, imported by `GraphqlModule` under the
/// module's own identity, so an application importing the same module elsewhere has one instance.
#[derive(Clone)]
pub(crate) struct EngineModule {
    identity: ModuleIdentity,
    module: Arc<dyn Module>,
}

impl Module for EngineModule {
    fn identity(&self) -> ModuleIdentity {
        self.identity.clone()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        self.module.register(m);
    }
}

impl GraphqlConfig {
    /// The endpoint at `path`, the engine bound unqualified, the playground on in debug builds.
    pub fn at(path: impl Into<Cow<'static, str>>) -> Self {
        GraphqlConfig {
            path: path.into(),
            subscriptions: None,
            playground: cfg!(debug_assertions),
            connection_init_timeout: Bound::Default,
            engine_module: None,
            _engine: PhantomData,
        }
    }
}

impl<Q> GraphqlConfig<Q> {
    /// Serves graphql-transport-ws at `path`, the gateway `ulo-graphql-ws` writes, mounted by this
    /// module beside the endpoint; the playground connects its subscriptions to it. The
    /// application imports `WsModule` once for the gateway to be reachable.
    pub fn subscriptions(mut self, path: impl Into<Cow<'static, str>>) -> Self {
        self.subscriptions = Some(path.into());
        self
    }

    /// Serves the engine's playground on a GET with `Accept: text/html` and no `query`:
    /// `cfg!(debug_assertions)` unset.
    pub fn playground(mut self, enabled: bool) -> Self {
        self.playground = enabled;
        self
    }

    /// How long a subscription connection may go without `connection_init` before it closes with
    /// 4408: 3 seconds at `Bound::Default`, the reference server's; `Bound::Unbounded` waits
    /// indefinitely. `Bound::After(Duration::ZERO)` is refused when the application wires.
    pub fn connection_init_timeout(mut self, timeout: Bound) -> Self {
        self.connection_init_timeout = timeout;
        self
    }

    /// Serves the engine bound as `dyn Engine @ E`.
    pub fn engine<E: 'static>(self) -> GraphqlConfig<E> {
        GraphqlConfig {
            path: self.path,
            subscriptions: self.subscriptions,
            playground: self.playground,
            connection_init_timeout: self.connection_init_timeout,
            engine_module: self.engine_module,
            _engine: PhantomData,
        }
    }

    /// Reads the engine from `module`, which binds it and exports `dyn Engine` (or
    /// `dyn Engine @ Q`): `GraphqlModule` imports `module`, so neither needs to be global. The
    /// module's identity is its own, so importing an equal module elsewhere in the application
    /// is the same instance. The engine's context resolves in `module`, where the engine is
    /// bound. Unset, the engine has to be visible from `GraphqlModule` some other way, through a
    /// global module's exports.
    pub fn engine_from<M: Module>(mut self, module: M) -> Self {
        self.engine_module = Some(EngineModule { identity: module.identity(), module: Arc::new(module) });
        self
    }
}

impl<Q> Clone for GraphqlConfig<Q> {
    fn clone(&self) -> Self {
        GraphqlConfig {
            path: self.path.clone(),
            subscriptions: self.subscriptions.clone(),
            playground: self.playground,
            connection_init_timeout: self.connection_init_timeout,
            engine_module: self.engine_module.clone(),
            _engine: PhantomData,
        }
    }
}

impl<Q> PartialEq for GraphqlConfig<Q> {
    fn eq(&self, other: &Self) -> bool {
        self.path == other.path
            && self.subscriptions == other.subscriptions
            && self.playground == other.playground
            && self.connection_init_timeout == other.connection_init_timeout
            && self.engine_identity() == other.engine_identity()
    }
}

impl<Q> Eq for GraphqlConfig<Q> {}

impl<Q> Hash for GraphqlConfig<Q> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.path.hash(state);
        self.subscriptions.hash(state);
        self.playground.hash(state);
        self.connection_init_timeout.hash(state);
        self.engine_identity().hash(state);
    }
}

impl<Q> GraphqlConfig<Q> {
    fn engine_identity(&self) -> Option<&ModuleIdentity> {
        self.engine_module.as_ref().map(|engine| &engine.identity)
    }
}

/// `path` with one leading `/` and no trailing one, `/` itself kept: the form the playground
/// writes and the router joins.
fn normalize(path: &str) -> String {
    let trimmed = path.trim_matches('/');
    format!("/{trimmed}")
}
