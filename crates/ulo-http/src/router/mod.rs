//! The router every backend shares (transports DESIGN §3.2). Built in `prepare`; a duplicate
//! route, or two patterns that differ only in a parameter's name at one position, is a
//! `StartupError::Configure` naming both handlers.
//!
//! A path that matches nothing answers 404. A matching path with the wrong method answers 405
//! with `Allow`, which RFC 9110 requires. `HEAD` is answered from the `GET` handler with the body
//! omitted unless a `HEAD` handler exists. `OPTIONS` without a handler answers 204 with `Allow`;
//! a CORS preflight never reaches routing.

pub(crate) mod pattern;

use std::any::type_name;
use std::sync::Arc;
use std::time::Duration;

use http::{HeaderValue, Method};
use ulo::{MetaTier, Metadata, MountedHandler, TypeName};

use crate::__private::{HandlerFn, HttpHandler};
use crate::cx::PathParams;
use crate::limits::{BodyLimit, Timeout};
use crate::pre_dispatch::{ScopedStage, Stage};
use crate::router::pattern::Pattern;
use crate::server::{Failure, Names};
use crate::transport::Http;

/// Every route, by pattern, each with its methods.
pub(crate) struct Router {
    pub(crate) routes: Vec<RouteEntry>,
}

/// One pattern and the handlers answering it, by method.
pub(crate) struct RouteEntry {
    pub(crate) pattern: Pattern,
    pub(crate) methods: Vec<(Method, Arc<RouteTarget>)>,
    /// The `Allow` of a 405, and of an `OPTIONS` answered without a handler.
    allow: HeaderValue,
    /// The pattern as the host sees it, every target's `route`: `Routing::Options`.
    route: Arc<str>,
}

/// One matched handler, as `prepare` built it.
pub(crate) struct RouteTarget {
    pub(crate) handler: MountedHandler<Http>,
    pub(crate) call: HandlerFn,
    /// The pattern as written, the controller's prefix applied, for `HttpCx::route`.
    pub(crate) pattern: Arc<str>,
    /// `pattern` under the embedding's mount prefix, as the host sees it: the span's `http.route`
    /// and `Routing::Matched`. The same `Arc` when the app is not nested.
    pub(crate) route: Arc<str>,
    pub(crate) body_limit: u64,
    /// The route's `#[meta(Timeout(..))]` as a duration, `None` for no timeout.
    pub(crate) timeout: Option<Duration>,
    /// The pre-dispatch entries whose scope covers this route, middleware and tower layers in
    /// declaration order, composed once here; routes with the same set share one stack.
    pub(crate) stage: Arc<ScopedStage>,
}

/// What routing a request decided.
pub(crate) enum Routed<'r> {
    Found {
        target: &'r Arc<RouteTarget>,
        params: PathParams,
        /// A `HEAD` request answered by the `GET` handler: the body is dropped when written.
        head_from_get: bool,
    },
    NotFound,
    /// A path that matches with no handler for the method: 405 with this `Allow`.
    MethodNotAllowed { allow: HeaderValue },
    /// `OPTIONS` on a matching path with no `OPTIONS` handler: 204 with this `Allow`, `route` the
    /// pattern as the host sees it.
    Options { allow: HeaderValue, route: Arc<str> },
}

impl Router {
    /// The table for `handlers`, each `HttpHandler`'s pattern joined to its controller's prefix,
    /// with each `Path<T>` check run against its route and each route's scoped pre-dispatch stage
    /// taken from `stage`. `mount` is the embedding's normalized prefix, empty when the app is not
    /// nested, which only the recorded route carries. Every failure is returned, not the first.
    ///
    /// The routes are ordered by precedence, so among patterns matching one path the first is the
    /// most specific: compared left to right, a static segment before a parameter, a parameter
    /// before a rest.
    pub(crate) fn build(
        handlers: &[MountedHandler<Http>],
        default_body_limit: u64,
        stage: &Stage,
        mount: &str,
    ) -> Result<Router, Vec<Failure>> {
        let mut errors = Vec::new();
        let mut groups: Vec<Group> = Vec::new();
        let mut zeros: Vec<ZeroTimeout> = Vec::new();
        for handler in handlers {
            let who = Who::of(handler);
            let Some(http) = handler.handler::<HttpHandler>() else {
                errors.push(Failure::naming(vec![who.controller], move |names| {
                    format!("{} is mounted for HTTP without an HTTP handler value", who.text(names))
                }));
                continue;
            };
            let info = handler.info();
            let text = match info.prefix() {
                Some(prefix) => Pattern::join(prefix, http.path),
                None => http.path.to_owned(),
            };
            let pattern = match Pattern::parse(&text) {
                Ok(pattern) => pattern,
                Err(error) => {
                    errors.push(Failure::naming(vec![who.controller], move |names| format!("{}: {error}", who.text(names))));
                    continue;
                }
            };
            let names: Vec<&str> = pattern.param_names().collect();
            for check in &http.path_checks {
                if let Err(reason) = (check.check)(&names) {
                    let (ty, pattern) = (check.ty, pattern.to_string());
                    errors.push(Failure::naming(vec![who.controller, ty], move |names| {
                        format!("{}: `Path<{}>` does not fit the route `{pattern}`: {reason}", who.text(names), names.of(ty))
                    }));
                }
            }
            let timeout = info.meta::<Timeout>().copied();
            if timeout == Some(Timeout::after(Duration::ZERO)) {
                let handler = declared_on_method(info.metadata()).then_some(who.name);
                let route = format!("`{} {pattern}`", http.method());
                match zeros.iter_mut().find(|zero| zero.controller == who.controller && zero.handler == handler) {
                    Some(zero) => zero.routes.push(route),
                    None => zeros.push(ZeroTimeout { controller: who.controller, handler, routes: vec![route] }),
                }
            }
            let route = if mount.is_empty() { Arc::clone(&pattern.raw) } else { Arc::from(Pattern::join(mount, &pattern.raw)) };
            let target = Arc::new(RouteTarget {
                handler: handler.clone(),
                call: Arc::clone(&http.call),
                pattern: Arc::clone(&pattern.raw),
                route,
                body_limit: info.meta::<BodyLimit>().map_or(default_body_limit, |limit| limit.0),
                timeout: timeout.and_then(Timeout::duration),
                stage: stage.scoped_for(&pattern),
            });
            let method = http.method().clone();
            match groups.iter_mut().find(|group| group.pattern.conflicts(&pattern)) {
                Some(group) if group.pattern.raw != pattern.raw => {
                    let other = group.targets[0].2;
                    let (taken, pattern) = (group.pattern.to_string(), pattern.to_string());
                    errors.push(Failure::naming(vec![other.controller, who.controller], move |names| {
                        format!(
                            "the routes `{taken}` of {} and `{pattern}` of {} differ only in parameter names; one position takes one name",
                            other.text(names),
                            who.text(names)
                        )
                    }));
                }
                Some(group) => match group.targets.iter().find(|(existing, _, _)| *existing == method) {
                    Some((_, _, other)) => {
                        let (other, pattern) = (*other, pattern.to_string());
                        errors.push(Failure::naming(vec![other.controller, who.controller], move |names| {
                            format!("{} and {} both answer `{method} {pattern}`", other.text(names), who.text(names))
                        }));
                    }
                    None => group.targets.push((method, target, who)),
                },
                None => groups.push(Group { pattern, targets: vec![(method, target, who)] }),
            }
        }
        errors.extend(zeros.into_iter().map(ZeroTimeout::failure));
        if !errors.is_empty() {
            return Err(errors);
        }
        let mut routes: Vec<RouteEntry> = groups
            .into_iter()
            .map(|group| {
                let methods: Vec<(Method, Arc<RouteTarget>)> = group.targets.into_iter().map(|(method, target, _)| (method, target)).collect();
                let allow = allow_of(&methods);
                // A group holds one target at least, and its targets share one pattern.
                let route = Arc::clone(&methods[0].1.route);
                RouteEntry { pattern: group.pattern, methods, allow, route }
            })
            .collect();
        routes.sort_by_cached_key(|entry| entry.pattern.precedence());
        Ok(Router { routes })
    }

    /// The most specific pattern matching `path` decides: its handler for `method`, then a `GET`
    /// handler for a `HEAD`, then 204 with `Allow` for an `OPTIONS`, then 405 with `Allow`. A
    /// less specific pattern holding a handler for the method is not consulted, since the path
    /// names the resource and the method is checked against that resource.
    pub(crate) fn route(&self, method: &Method, path: &str) -> Routed<'_> {
        let path = pattern::normalize(path);
        let pieces = pattern::split(path);
        for entry in &self.routes {
            let Some(params) = entry.pattern.match_split(path, &pieces) else {
                continue;
            };
            if let Some(target) = entry.target(method) {
                return Routed::Found { target, params, head_from_get: false };
            }
            if *method == Method::HEAD {
                if let Some(target) = entry.target(&Method::GET) {
                    return Routed::Found { target, params, head_from_get: true };
                }
            }
            if *method == Method::OPTIONS {
                return Routed::Options { allow: entry.allow.clone(), route: Arc::clone(&entry.route) };
            }
            return Routed::MethodNotAllowed { allow: entry.allow.clone() };
        }
        Routed::NotFound
    }
}

impl RouteEntry {
    fn target(&self, method: &Method) -> Option<&Arc<RouteTarget>> {
        self.methods.iter().find(|(candidate, _)| candidate == method).map(|(_, target)| target)
    }
}

/// The routes sharing one pattern while the table is built, each with the handler it names in
/// errors.
struct Group {
    pattern: Pattern,
    targets: Vec<(Method, Arc<RouteTarget>, Who)>,
}

/// A handler as a route-table failure names it, `` `Users::list` ``.
#[derive(Clone, Copy)]
struct Who {
    controller: TypeName,
    name: &'static str,
}

impl Who {
    fn of(handler: &MountedHandler<Http>) -> Who {
        Who { controller: handler.controller().key().type_name(), name: handler.name() }
    }

    fn text(&self, names: &Names) -> String {
        format!("`{}::{}`", names.of(self.controller), self.name)
    }
}

/// One zero `#[meta(Timeout(..))]` declaration and the routes that run with it, in declaration
/// order: one failure per declaration, however many routes it reaches.
struct ZeroTimeout {
    controller: TypeName,
    /// The handler declaring it, `None` for the `#[routes]` impl.
    handler: Option<&'static str>,
    routes: Vec<String>,
}

impl ZeroTimeout {
    fn failure(self) -> Failure {
        let ZeroTimeout { controller, handler, routes } = self;
        let routes = routes.join(", ");
        Failure::naming(vec![controller], move |names| {
            let declared = match handler {
                Some(name) => format!("on the handler {}", Who { controller, name }.text(names)),
                None => format!("on the `#[routes]` impl of `{}`", names.of(controller)),
            };
            format!(
                "`#[meta(Timeout(..))]` {declared} would cancel every request on {routes}; write `Timeout::OFF` to turn the \
                 timeout off"
            )
        })
    }
}

/// Whether the `Timeout` a handler takes is the method's own. The method's declaration wins over
/// the controller's, so it is the method's whenever the method has one.
fn declared_on_method(metadata: &Metadata) -> bool {
    metadata.entries().any(|(name, tier)| tier == MetaTier::Method && name == type_name::<Timeout>())
}

/// The methods a pattern answers, in declaration order: `HEAD` beside `GET` when only `GET` is
/// declared, since `GET` answers it, and `OPTIONS`, which the router answers itself.
fn allow_of(methods: &[(Method, Arc<RouteTarget>)]) -> HeaderValue {
    let mut names: Vec<&str> = methods.iter().map(|(method, _)| method.as_str()).collect();
    if !names.contains(&"HEAD") {
        if let Some(get) = names.iter().position(|name| *name == "GET") {
            names.insert(get + 1, "HEAD");
        }
    }
    if !names.contains(&"OPTIONS") {
        names.push("OPTIONS");
    }
    // Method names are tokens, which a header value always admits.
    HeaderValue::from_str(&names.join(", ")).unwrap_or_else(|_| HeaderValue::from_static("OPTIONS"))
}
