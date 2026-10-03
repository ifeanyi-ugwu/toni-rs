//! The one middleware stage, pre-dispatch, declared in module metadata (transports DESIGN §3.3,
//! §3.4).
//!
//! It runs in two sub-steps, because a path rewrite has to happen before route matching while
//! scoping by route pattern needs the match:
//! 1. Unscoped entries, in the order written, modules in collection order. They can rewrite the
//!    path, they see misses, and they can answer without calling `next`.
//! 2. The route is matched and the execution routed to the controller's module
//!    (`Execution::route_to`); then the entries whose scope covers the route run.
//!
//! Then `dispatch`. Both sub-steps run inside the error chain: an entry's `Err`, a tower layer's,
//! reaches the error handlers as a `BoxError` and its panic as `PanicRecovered`; on a miss only the
//! global error handlers apply. A request that arrives with no execution to open, during the drain,
//! never reaches the stage.
//!
//! There is no per-module middleware: auth for a group of routes is a scoped entry, which runs
//! before guards and can set `CurrentUser`; logging or transforming a module's responses is an
//! interceptor; rejecting a request is a guard.

use std::borrow::Cow;
use std::panic::Location;
use std::sync::Arc;

use bytes::Bytes;
use ulo::{BoxError, BoxFuture, Dependencies, LookupError, Meta, ModuleRef};

use crate::body::HttpBody;
use crate::middleware::{ErasedMiddleware, Middleware};
use crate::tower_bridge::{ErasedLayer, Service};

/// The pre-dispatch stage's entries one module declares:
///
/// ```ignore
/// m.meta::<ulo_http::PreDispatch>()
///     .apply_value(Cors::new().allow_origin("https://app.example").allow_credentials(true))
///     .apply::<RequestId>()                                           // unscoped: every request, misses included
///     .layer(TraceLayer::new_for_http())                              // tower, unscoped
///     .layer_for(["/files/*"], RequestBodyLimitLayer::new(50 * MB))   // tower, scoped by route pattern
///     .apply_for::<ApiKeyAuth>(["/admin/*"]).exclude(["/admin/health"]);
/// ```
///
/// A scope is a route pattern whose last segment may be `*`, matched against route patterns when
/// the server prepares; `exclude` removes routes from the entry written before it. The
/// middleware types an entry names are declared as its dependencies, so `wire()` checks that the
/// declaring module sees them.
#[derive(Default)]
pub struct PreDispatch {
    pub(crate) entries: Vec<Entry>,
}

/// One entry as declared.
pub(crate) struct Entry {
    pub(crate) step: Step,
    /// Empty for an unscoped entry.
    pub(crate) scope: Vec<Cow<'static, str>>,
    pub(crate) exclude: Vec<Cow<'static, str>>,
    /// `Dep<M>` for a by-type middleware; nothing otherwise.
    pub(crate) dependencies: fn(&mut Dependencies),
    /// A check `prepare` runs, for a value it can validate: CORS refusing `*` with credentials.
    pub(crate) check: Option<Arc<dyn Fn() -> Result<(), BoxError> + Send + Sync>>,
    pub(crate) location: &'static Location<'static>,
}

/// What an entry runs.
pub(crate) enum Step {
    /// Resolved per request through the declaring module, carrying the request's execution.
    ByType(fn(ModuleRef) -> BoxFuture<'static, Result<Arc<dyn ErasedMiddleware>, LookupError>>),
    Value(Arc<dyn ErasedMiddleware>),
    Layer(Arc<dyn ErasedLayer>),
}

impl Meta for PreDispatch {
    fn dependencies(&self, d: &mut Dependencies) {
        for entry in &self.entries {
            (entry.dependencies)(d);
        }
    }
}

impl PreDispatch {
    /// A middleware by type, unscoped: every request, misses included.
    #[track_caller]
    pub fn apply<M: Middleware>(&mut self) -> &mut Self {
        let _ = std::marker::PhantomData::<M>;
        todo!("push a `Step::ByType` resolving `Dep<M>` through the module, `dependencies` declaring it")
    }

    /// A middleware by value, built once and shared, unscoped. A `Cors` value is checked in
    /// `prepare`.
    #[track_caller]
    pub fn apply_value<M: Middleware>(&mut self, middleware: M) -> &mut Self {
        let _ = middleware;
        todo!("push a `Step::Value`; record a check when `M` is `Cors`")
    }

    /// A middleware by type, for the routes `patterns` cover.
    #[track_caller]
    pub fn apply_for<M: Middleware>(&mut self, patterns: impl IntoIterator<Item = impl Into<Cow<'static, str>>>) -> &mut Self {
        let _ = (patterns.into_iter().map(Into::into).collect::<Vec<Cow<'static, str>>>(), std::marker::PhantomData::<M>);
        todo!("as `apply`, scoped")
    }

    /// A tower layer over [`Service`], unscoped: it wraps the whole stage before matching, so it
    /// can rewrite and it sees misses. Composed once, in `prepare`, never per request.
    ///
    /// A response the layer builds itself, tower-http's auth answering 401 for example, is a
    /// response and not an error, so error handlers do not see it.
    #[track_caller]
    pub fn layer<L, S, B>(&mut self, layer: L) -> &mut Self
    where
        L: tower::Layer<Service, Service = S> + Send + Sync + 'static,
        S: tower::Service<http::Request<HttpBody>, Response = http::Response<B>> + Clone + Send + Sync + 'static,
        S::Error: Into<BoxError>,
        S::Future: Send + 'static,
        B: http_body::Body<Data = Bytes> + Send + 'static,
        B::Error: Into<BoxError>,
    {
        let _ = layer;
        todo!("push a `Step::Layer` erasing `L`")
    }

    /// A tower layer for the routes `patterns` cover. For each route, `prepare` builds its own
    /// stack from the layers whose scope covers it, in declaration order; routes with the same set
    /// share one stack.
    #[track_caller]
    pub fn layer_for<L, S, B>(&mut self, patterns: impl IntoIterator<Item = impl Into<Cow<'static, str>>>, layer: L) -> &mut Self
    where
        L: tower::Layer<Service, Service = S> + Send + Sync + 'static,
        S: tower::Service<http::Request<HttpBody>, Response = http::Response<B>> + Clone + Send + Sync + 'static,
        S::Error: Into<BoxError>,
        S::Future: Send + 'static,
        B: http_body::Body<Data = Bytes> + Send + 'static,
        B::Error: Into<BoxError>,
    {
        let _ = (patterns.into_iter().map(Into::into).collect::<Vec<Cow<'static, str>>>(), layer);
        todo!("as `layer`, scoped")
    }

    /// Removes the routes `patterns` cover from the entry written before it. With no entry
    /// before it, `prepare` reports the call.
    pub fn exclude(&mut self, patterns: impl IntoIterator<Item = impl Into<Cow<'static, str>>>) -> &mut Self {
        if let Some(entry) = self.entries.last_mut() {
            entry.exclude.extend(patterns.into_iter().map(Into::into));
        }
        self
    }
}

/// Every module's entries, in collection order, as one stage, each with the module that declared
/// it and its index in that module's `PreDispatch::entries`.
pub(crate) struct Stage {
    pub(crate) unscoped: Vec<(ModuleRef, Arc<PreDispatch>, usize)>,
    pub(crate) scoped: Vec<(ModuleRef, Arc<PreDispatch>, usize)>,
    /// The scoped stages handed out so far, by the entries they hold, so equal sets share one.
    pub(crate) shared: std::sync::Mutex<Vec<Arc<ScopedStage>>>,
}

/// The entries one route runs after it matched: those whose scope covers it, minus exclusions,
/// in declaration order, each with its declaring module. Built once per distinct set in `prepare`.
pub(crate) struct ScopedStage {
    pub(crate) steps: Vec<(ModuleRef, Arc<PreDispatch>, usize)>,
}

impl Stage {
    /// The stage from `module_meta::<PreDispatch>()`, every scope and exclusion parsed and every
    /// value check run; every failure returned, for `StartupError::Configure`.
    pub(crate) fn build(metas: Vec<(ModuleRef, Arc<PreDispatch>)>) -> Result<Stage, Vec<String>> {
        let _ = metas;
        todo!("parse scopes and exclusions, run checks, keep unscoped entries in order")
    }

    /// The scoped stage of the route `pattern`, the same `Arc` for routes covered by the same set.
    pub(crate) fn scoped_for(&self, pattern: &crate::router::pattern::Pattern) -> Arc<ScopedStage> {
        let _ = pattern;
        todo!("the scoped entries covering `pattern`, minus exclusions; shared between equal sets")
    }
}
