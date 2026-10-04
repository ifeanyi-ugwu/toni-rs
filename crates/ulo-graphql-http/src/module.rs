//! `GraphqlModule` and its configuration.

use std::borrow::Cow;
use std::marker::PhantomData;

use ulo::{Module, ModuleDef, ModuleIdentity};

/// Serves one engine at one path: the engine bound under `dyn Engine`, or `dyn Engine @ Q` once
/// [`GraphqlConfig::engine`] names a qualifier, so a second `GraphqlModule` at another path serves
/// a second schema with no GraphQL rule added.
pub struct GraphqlModule<Q = ()> {
    pub(crate) config: GraphqlConfig<Q>,
}

impl<Q: Send + Sync + 'static> GraphqlModule<Q> {
    pub fn for_root(config: GraphqlConfig<Q>) -> Self {
        GraphqlModule { config }
    }
}

impl<Q: Send + Sync + 'static> Module for GraphqlModule<Q> {
    fn identity(&self) -> ModuleIdentity {
        todo!()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        let _ = (m, &self.config);
        todo!()
    }
}

/// Where the endpoint is served and what it offers.
pub struct GraphqlConfig<Q = ()> {
    pub(crate) path: Cow<'static, str>,
    pub(crate) subscriptions: Option<Cow<'static, str>>,
    pub(crate) playground: bool,
    pub(crate) _engine: PhantomData<fn() -> Q>,
}

impl GraphqlConfig {
    /// The endpoint at `path`, the engine bound unqualified, the playground on in debug builds.
    pub fn at(path: impl Into<Cow<'static, str>>) -> Self {
        GraphqlConfig { path: path.into(), subscriptions: None, playground: cfg!(debug_assertions), _engine: PhantomData }
    }
}

impl<Q> GraphqlConfig<Q> {
    /// The graphql-transport-ws endpoint's path, which the playground connects its subscriptions
    /// to.
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

    /// Serves the engine bound as `dyn Engine @ E`.
    pub fn engine<E: 'static>(self) -> GraphqlConfig<E> {
        GraphqlConfig { path: self.path, subscriptions: self.subscriptions, playground: self.playground, _engine: PhantomData }
    }
}
