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
//! Then `dispatch`. Both sub-steps run inside the error chain: an entry's `Err`, a middleware's or
//! a tower layer's, reaches the error handlers as a `BoxError` and its panic as `PanicRecovered`;
//! on a miss only the global error handlers apply. A request that arrives with no execution to
//! open, during the drain, never reaches the stage.
//!
//! There is no per-module middleware: auth for a group of routes is a scoped entry, which runs
//! before guards and can set `CurrentUser`; logging or transforming a module's responses is an
//! interceptor; rejecting a request is a guard.

use std::any::Any;
use std::borrow::Cow;
use std::panic::Location;
use std::sync::{Arc, Mutex, PoisonError};

use bytes::Bytes;
use ulo::{BoxError, BoxFuture, Dep, Dependencies, DispatchStage, ExecutionRef, Key, LookupError, Meta, ModuleRef};

use crate::body::HttpBody;
use crate::cors::Cors;
use crate::cx::{MatchedRoute, PathParams};
use crate::middleware::{ErasedMiddleware, Middleware, Next};
use crate::render;
use crate::request::{ConnInfo, Request};
use crate::response::Response;
use crate::router::RouteTarget;
use crate::router::pattern::{Pattern, ScopePattern};
use crate::service::{ServiceInner, merge_headers};
use crate::tower_bridge::{Continuation, ErasedLayer, LayerOf, LayeredService, Service};
use crate::transport::RequestHead;

/// The pre-dispatch stage's entries one module declares:
///
/// ```ignore
/// m.meta::<ulo_http::PreDispatch>()
///     .apply_value(Cors::new().allow_origin("https://app.example").allow_credentials(true))
///     .apply::<RequestId>()                                           // unscoped: every request, misses included
///     .layer(TraceLayer::new_for_http())                              // tower, unscoped
///     .layer(HostAuthLayer::new()).supplies::<CurrentUser>()          // inserts `CurrentUser`
///     .layer_for(["/files/*"], RequestBodyLimitLayer::new(50 * MB))   // tower, scoped by route pattern
///     .apply_for::<ApiKeyAuth>(["/admin/*"]).exclude(["/admin/health"])
///     .adopt::<CurrentUser>();                                        // into the execution, as `Ext<CurrentUser>`
/// ```
///
/// A scope is a route pattern whose last segment may be `*`, matched against route patterns when
/// the server prepares; `exclude` removes routes from the entry written before it. On an unscoped
/// entry, which runs before routing, `exclude` is matched against the request's path as that entry
/// receives it. The middleware types an entry names are declared as its dependencies, so `wire()`
/// checks that the declaring module sees them.
///
/// `prepare` refuses a scope or an exclusion that does not parse, a scoped entry naming no pattern,
/// an `exclude` or a `supplies` written before any entry, a `supplies` written after an `adopt`,
/// and a `Cors` value the Fetch specification forbids.
#[derive(Default)]
pub struct PreDispatch {
    pub(crate) entries: Vec<Entry>,
    /// Where `exclude` was called with no entry before it, for `prepare` to report.
    pub(crate) stray_excludes: Vec<&'static Location<'static>>,
    /// Where `supplies` was called with no entry before it, for `prepare` to report.
    pub(crate) stray_supplies: Vec<&'static Location<'static>>,
    /// Where `supplies` was called after an `adopt` entry, for `prepare` to report. The
    /// declaration is kept off the entry, so the embedding's check never counts it.
    pub(crate) adopt_supplies: Vec<&'static Location<'static>>,
}

/// One entry as declared.
pub(crate) struct Entry {
    pub(crate) step: Step,
    /// Written with `apply_for` or `layer_for`: it runs for the routes `scope` covers, and an
    /// empty `scope` covers none.
    pub(crate) scoped: bool,
    pub(crate) scope: Vec<Cow<'static, str>>,
    pub(crate) exclude: Vec<Cow<'static, str>>,
    /// What the `supplies` calls written after it declare this entry inserts: an embedding
    /// declaring `host_extensions: false` reads them where the entry runs, for the routes its
    /// scope covers or, unscoped, for every route and every `adopt` written after it.
    pub(crate) supplies: Vec<Supply>,
    /// `Dep<M>` for a by-type middleware; nothing otherwise.
    pub(crate) dependencies: fn(&mut Dependencies),
    /// A check `prepare` runs, for a value it can validate: CORS refusing `*` with credentials.
    pub(crate) check: Option<Check>,
    pub(crate) location: &'static Location<'static>,
}

pub(crate) type Check = Arc<dyn Fn() -> Result<(), BoxError> + Send + Sync>;

/// One `supplies::<T>()`: the type, and where it was written, for a refusal to name.
pub(crate) struct Supply {
    pub(crate) ty: Key,
    pub(crate) location: &'static Location<'static>,
}

/// What an entry runs.
pub(crate) enum Step {
    /// Resolved per request through the declaring module, carrying the request's execution.
    ByType(fn(ModuleRef) -> BoxFuture<'static, Result<Arc<dyn ErasedMiddleware>, LookupError>>),
    Value(Arc<dyn ErasedMiddleware>),
    Layer(Arc<dyn ErasedLayer>),
    /// Copies one type from the request's `http::Extensions` into the execution's.
    Adopt(Adopt, Key),
}

/// What an `adopt::<T>()` entry runs.
pub(crate) type Adopt = fn(&http::request::Parts, &ExecutionRef);

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
        self.push(Step::ByType(resolve::<M>), None, declare::<M>, None)
    }

    /// A middleware by value, built once and shared, unscoped. A `Cors` value is checked in
    /// `prepare`.
    #[track_caller]
    pub fn apply_value<M: Middleware>(&mut self, middleware: M) -> &mut Self {
        let check = (&middleware as &dyn Any).downcast_ref::<Cors>().map(|cors| -> Check {
            let cors = cors.clone();
            Arc::new(move || cors.check().map_err(BoxError::from))
        });
        self.push(Step::Value(Arc::new(middleware)), None, declare_nothing, check)
    }

    /// A middleware by type, for the routes `patterns` cover.
    #[track_caller]
    pub fn apply_for<M: Middleware>(&mut self, patterns: impl IntoIterator<Item = impl Into<Cow<'static, str>>>) -> &mut Self {
        let scope = patterns.into_iter().map(Into::into).collect();
        self.push(Step::ByType(resolve::<M>), Some(scope), declare::<M>, None)
    }

    /// A tower layer over [`Service`], unscoped: it wraps the whole stage before matching, so it
    /// can rewrite and it sees misses. Composed once, in `prepare`, never per request.
    ///
    /// A response the layer builds itself, tower-http's auth answering 401 for example, is a
    /// response and not an error, so error handlers do not see it. An `Err` its service returns
    /// reaches them as a middleware's `Err` does.
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
        self.push(Step::Layer(Arc::new(LayerOf(layer))), None, declare_nothing, None)
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
        let scope = patterns.into_iter().map(Into::into).collect();
        self.push(Step::Layer(Arc::new(LayerOf(layer))), Some(scope), declare_nothing, None)
    }

    /// Copies the value `T` from the request's `http::Extensions`, where a host's middleware or a
    /// tower layer before this entry put it, into the execution's extensions, so guards,
    /// interceptors and execution-scoped services read it as `Ext<T>`. Unscoped: it runs for
    /// every request, misses included, at its place in declaration order. A request carrying no
    /// `T` passes unchanged, and `Ext<T>` then fails with `LookupError::NotFound`.
    ///
    /// That is how a host's auth layer hands its user to the app's guards; a handler reads a host
    /// value directly with `Host<T>`. An embedding declaring `host_extensions: false` refuses it
    /// unless `T` is supplied before it runs: by `Embedded::forward::<T>(..)`, or by an unscoped
    /// entry written before it and declared with `.supplies::<T>()`.
    #[track_caller]
    pub fn adopt<T: Clone + Send + Sync + 'static>(&mut self) -> &mut Self {
        self.push(Step::Adopt(adopt::<T>, Key::of::<T, ()>()), None, declare_nothing, None)
    }

    /// Declares that the entry written before it puts `T` in the request's `http::Extensions`,
    /// as a tower layer authenticating the request does. Runs nothing, on a backend or embedded.
    ///
    /// An embedding whose adapter declares `host_extensions: false` refuses a handler reading
    /// `Host<T>` and an `adopt::<T>()` entry unless `T` reaches them, and reads the declaration
    /// where the entry runs. After a scoped entry it reaches the handlers of the routes that
    /// entry covers; after an unscoped one, every handler, and the `adopt` entries written after
    /// it. A scoped entry runs after routing, so its declaration reaches no `adopt`. A value the
    /// host keeps is copied in by `Embedded::forward::<T>(..)`, which reaches everything.
    ///
    /// An unscoped entry's `exclude` is not consulted: it matches the path as that entry receives
    /// it, which an entry after it may rewrite before routing. A supply after an excluding entry
    /// therefore lifts the refusal for the routes it excludes too, and a handler there reading
    /// `Host<T>` answers 500 (`HostMissing`) per request. Put the supplying entry where its
    /// exclusion matches the routes that read the value: an `exclude` covering none of them, or
    /// a scoped entry, whose exclusions are applied to routes when the server prepares.
    ///
    /// `prepare` reports the call, on a backend as well, when no entry is before it, and when the
    /// entry before it is an `adopt`, which inserts nothing into the request.
    #[track_caller]
    pub fn supplies<T: Clone + Send + Sync + 'static>(&mut self) -> &mut Self {
        let supply = Supply { ty: Key::of::<T, ()>(), location: Location::caller() };
        match self.entries.last_mut() {
            Some(Entry { step: Step::Adopt(..), .. }) => self.adopt_supplies.push(supply.location),
            Some(entry) => entry.supplies.push(supply),
            None => self.stray_supplies.push(supply.location),
        }
        self
    }

    /// Removes the routes `patterns` cover from the entry written before it. With no entry
    /// before it, `prepare` reports the call.
    #[track_caller]
    pub fn exclude(&mut self, patterns: impl IntoIterator<Item = impl Into<Cow<'static, str>>>) -> &mut Self {
        match self.entries.last_mut() {
            Some(entry) => entry.exclude.extend(patterns.into_iter().map(Into::into)),
            None => self.stray_excludes.push(Location::caller()),
        }
        self
    }

    #[track_caller]
    fn push(
        &mut self,
        step: Step,
        scope: Option<Vec<Cow<'static, str>>>,
        dependencies: fn(&mut Dependencies),
        check: Option<Check>,
    ) -> &mut Self {
        self.entries.push(Entry {
            step,
            scoped: scope.is_some(),
            scope: scope.unwrap_or_default(),
            exclude: Vec::new(),
            supplies: Vec::new(),
            dependencies,
            check,
            location: Location::caller(),
        });
        self
    }
}

fn resolve<M: Middleware>(module: ModuleRef) -> BoxFuture<'static, Result<Arc<dyn ErasedMiddleware>, LookupError>> {
    Box::pin(async move {
        let middleware = module.get::<M>().await?;
        let middleware: Arc<dyn ErasedMiddleware> = middleware.into_arc();
        Ok::<_, LookupError>(middleware)
    })
}

fn declare<M: Middleware>(d: &mut Dependencies) {
    d.add::<Dep<M>>();
}

fn declare_nothing(_: &mut Dependencies) {}

fn adopt<T: Clone + Send + Sync + 'static>(head: &http::request::Parts, exec: &ExecutionRef) {
    if let Some(value) = head.extensions.get::<T>() {
        exec.extensions().insert(value.clone());
    }
}

/// Every module's entries, in collection order, as one stage, each scoped entry with the module
/// that declared it and its index in that module's `PreDispatch::entries`.
pub(crate) struct Stage {
    pub(crate) scoped: Vec<(ModuleRef, Arc<PreDispatch>, usize)>,
    /// The scoped stages handed out so far, by the entries they hold, so equal sets share one.
    pub(crate) shared: Mutex<Vec<Arc<ScopedStage>>>,
    /// The unscoped entries as they run, in order.
    run: Arc<[Runnable]>,
    /// `scoped` as it runs, each with the scopes it covers and excludes, index for index.
    covering: Vec<Covering>,
}

/// The entries one route runs after it matched: those whose scope covers it, minus exclusions,
/// in declaration order, each with its declaring module. Built once per distinct set in `prepare`.
pub(crate) struct ScopedStage {
    pub(crate) steps: Vec<(ModuleRef, Arc<PreDispatch>, usize)>,
    /// `steps` as they run.
    run: Arc<[Runnable]>,
}

impl ScopedStage {
    /// Whether this stage holds exactly `steps`: the same declarations, by identity.
    fn holds(&self, steps: &[(ModuleRef, Arc<PreDispatch>, usize)]) -> bool {
        self.steps.len() == steps.len()
            && self.steps.iter().zip(steps).all(|((_, held, at), (_, wanted, index))| Arc::ptr_eq(held, wanted) && at == index)
    }
}

struct Covering {
    runnable: Runnable,
    scope: Vec<ScopePattern>,
    exclude: Vec<ScopePattern>,
}

/// One entry as a request runs it, its tower layer composed.
#[derive(Clone)]
pub(crate) struct Runnable {
    module: ModuleRef,
    action: Action,
    /// The request paths an unscoped entry skips. A scoped entry's exclusions were applied when
    /// its route's stage was built, so it has none here.
    exclude: Vec<ScopePattern>,
}

#[derive(Clone)]
enum Action {
    ByType(fn(ModuleRef) -> BoxFuture<'static, Result<Arc<dyn ErasedMiddleware>, LookupError>>),
    Value(Arc<dyn ErasedMiddleware>),
    Layer(LayeredService),
    Adopt(Adopt),
}

impl Stage {
    /// The stage from `module_meta::<PreDispatch>()`, every scope and exclusion parsed and every
    /// value check run; every failure returned, for `StartupError::Configure`.
    pub(crate) fn build(metas: Vec<(ModuleRef, Arc<PreDispatch>)>) -> Result<Stage, Vec<String>> {
        let mut failures = Vec::new();
        let mut stage = Stage::empty();
        let mut run = Vec::new();
        for (module, meta) in metas {
            for location in &meta.stray_excludes {
                failures.push(format!("`exclude` at {location} follows no pre-dispatch entry, so it excludes nothing"));
            }
            for location in &meta.stray_supplies {
                failures.push(format!(
                    "`supplies` at {location} follows no pre-dispatch entry; it declares what the entry before it inserts, \
                     and a value the host keeps is copied in by the embedding with `Embedded::forward`"
                ));
            }
            for location in &meta.adopt_supplies {
                failures.push(format!(
                    "`supplies` at {location} follows an `adopt` entry: `adopt` copies a value out of the request and \
                     inserts none; declare the `supplies` after the entry that inserts it"
                ));
            }
            for (index, entry) in meta.entries.iter().enumerate() {
                if let Some(check) = &entry.check {
                    if let Err(err) = (**check)() {
                        failures.push(format!("pre-dispatch entry at {}: {err}", entry.location));
                    }
                }
                let exclude = scopes(&entry.exclude, entry.location, &mut failures);
                let action = match &entry.step {
                    Step::ByType(resolve) => Action::ByType(*resolve),
                    Step::Value(middleware) => Action::Value(Arc::clone(middleware)),
                    Step::Layer(layer) => Action::Layer(layer.layer(Service::default())),
                    Step::Adopt(copy, _) => Action::Adopt(*copy),
                };
                if !entry.scoped {
                    run.push(Runnable { module: module.clone(), action, exclude });
                    continue;
                }
                if entry.scope.is_empty() {
                    failures.push(format!("pre-dispatch entry at {} names no route pattern, so it covers no route", entry.location));
                }
                let scope = scopes(&entry.scope, entry.location, &mut failures);
                stage.scoped.push((module.clone(), Arc::clone(&meta), index));
                stage.covering.push(Covering {
                    runnable: Runnable { module: module.clone(), action, exclude: Vec::new() },
                    scope,
                    exclude,
                });
            }
        }
        if !failures.is_empty() {
            return Err(failures);
        }
        stage.run = run.into();
        Ok(stage)
    }

    /// A stage with no entries, for a `prepare` whose own stage failed, so the route table is
    /// still built and its failures reported beside the stage's.
    pub(crate) fn empty() -> Stage {
        Stage {
            scoped: Vec::new(),
            shared: Mutex::new(Vec::new()),
            run: Arc::from(Vec::new()),
            covering: Vec::new(),
        }
    }

    /// The scoped stage of the route `pattern`, the same `Arc` for routes covered by the same set.
    pub(crate) fn scoped_for(&self, pattern: &Pattern) -> Arc<ScopedStage> {
        let covered: Vec<usize> = self
            .covering
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                entry.scope.iter().any(|scope| scope.covers(pattern)) && !entry.exclude.iter().any(|scope| scope.covers(pattern))
            })
            .map(|(index, _)| index)
            .collect();
        let steps: Vec<(ModuleRef, Arc<PreDispatch>, usize)> = covered.iter().map(|&index| self.scoped[index].clone()).collect();
        let mut shared = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(stage) = shared.iter().find(|stage| stage.holds(&steps)) {
            return Arc::clone(stage);
        }
        let run = covered.iter().map(|&index| self.covering[index].runnable.clone()).collect();
        let stage = Arc::new(ScopedStage { steps, run });
        shared.push(Arc::clone(&stage));
        stage
    }

    /// The unscoped entries as a request runs them.
    pub(crate) fn runnable(&self) -> Arc<[Runnable]> {
        Arc::clone(&self.run)
    }
}

impl ScopedStage {
    /// The entries as a request runs them.
    pub(crate) fn runnable(&self) -> Arc<[Runnable]> {
        Arc::clone(&self.run)
    }
}

fn scopes(patterns: &[Cow<'static, str>], at: &Location<'_>, failures: &mut Vec<String>) -> Vec<ScopePattern> {
    patterns
        .iter()
        .filter_map(|pattern| match ScopePattern::parse(pattern) {
            Ok(scope) => Some(scope),
            Err(err) => {
                failures.push(format!("pre-dispatch entry at {at}: {err}"));
                None
            }
        })
        .collect()
}

/// What runs after an entry: the entries after it, then routing or dispatch.
pub(crate) type Rest = Box<dyn FnOnce(Request) -> BoxFuture<'static, Response> + Send>;

/// One request's sub-step, as an entry's failure is offered to the error handlers.
pub(crate) struct StageCx {
    pub(crate) service: Arc<ServiceInner>,
    pub(crate) exec: ExecutionRef,
    /// The head as the sub-step received it, for the context the error handlers read: the
    /// request itself is the failing entry's.
    pub(crate) head: Arc<RequestHead>,
    pub(crate) conn: ConnInfo,
    /// The matched route in the scoped sub-step, whose handler's tiers see a failure before the
    /// global ones; `None` in the unscoped one, where only the global ones apply.
    pub(crate) route: Option<(Arc<RouteTarget>, PathParams)>,
}

impl StageCx {
    async fn fail(&self, err: BoxError) -> Response {
        let route = self.route.as_ref().map(|(target, params)| MatchedRoute {
            handler: target.handler.clone(),
            pattern: Arc::clone(&target.pattern),
            params: params.clone(),
            body_limit: target.body_limit,
        });
        let cx = self.service.context(&self.exec, Arc::clone(&self.head), self.conn.clone(), route, None, None);
        let handler = self.route.as_ref().map(|(target, _)| &target.handler);
        let response = match ulo::recover(handler, &self.exec, &cx, err).await {
            Ok(response) => response,
            Err(err) => render::render_error(err, &self.exec, &self.service.config),
        };
        merge_headers(&cx, response)
    }
}

/// Runs `steps` from `from` on, then `end`. Each entry runs inside `AppHandle::catch_panic`, and
/// its `Err`, a middleware's or a layer's, or its panic is offered to the error handlers in its
/// place; the entries after it run inside it and have each been caught already, so what reaches
/// its catch is its own.
pub(crate) fn run(at: Arc<StageCx>, steps: Arc<[Runnable]>, from: usize, req: Request, end: Rest) -> BoxFuture<'static, Response> {
    let Some(index) = (from..steps.len()).find(|&index| !steps[index].skips(req.path())) else {
        return end(req);
    };
    let attempt = {
        let rest_at = Arc::clone(&at);
        let rest_steps = Arc::clone(&steps);
        let rest: Rest = Box::new(move |req| run(rest_at, rest_steps, index + 1, req, end));
        steps[index].start(&at.exec, req, rest)
    };
    Box::pin(async move {
        match at.service.app.catch_panic(DispatchStage::PreDispatch, attempt).await {
            Ok(response) => response,
            Err(err) => at.fail(err).await,
        }
    })
}

impl Runnable {
    fn skips(&self, path: &str) -> bool {
        self.exclude.iter().any(|scope| scope.covers_path(path))
    }

    fn start(&self, exec: &ExecutionRef, req: Request, rest: Rest) -> BoxFuture<'static, Result<Response, BoxError>> {
        match &self.action {
            Action::Value(middleware) => Box::pin(handle(Arc::clone(middleware), req, rest)),
            Action::ByType(resolve) => {
                let resolving = resolve(self.module.with_execution(exec));
                Box::pin(async move {
                    let middleware = resolving.await?;
                    handle(middleware, req, rest).await
                })
            }
            Action::Adopt(copy) => {
                copy(&req.head, exec);
                let answer = rest(req);
                Box::pin(async move { Ok::<_, BoxError>(answer.await) })
            }
            Action::Layer(service) => {
                let service = Arc::clone(service);
                Box::pin(async move {
                    let Request { head, body, conn, upgrade } = req;
                    let mut request = http::Request::from_parts(head, body);
                    // The layered service was built once and cannot hold this request's state:
                    // `Service::call` takes the continuation back out, with what the `http`
                    // request cannot carry.
                    request.extensions_mut().insert(Continuation::new(move |request: http::Request<HttpBody>| {
                        let (head, body) = request.into_parts();
                        rest(Request { head, body, conn, upgrade })
                    }));
                    (*service)(request).await
                })
            }
        }
    }
}

async fn handle(middleware: Arc<dyn ErasedMiddleware>, req: Request, rest: Rest) -> Result<Response, BoxError> {
    let next = Next::new(move |req| {
        let answer = rest(req);
        Box::pin(async move { Ok::<_, BoxError>(answer.await) })
    });
    ErasedMiddleware::handle(&*middleware, req, next).await
}
