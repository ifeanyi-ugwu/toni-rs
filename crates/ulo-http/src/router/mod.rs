//! The router every backend shares (transports DESIGN §3.2). Built in `prepare`; a duplicate
//! route, or two patterns that differ only in a parameter's name at one position, is a
//! `StartupError::Configure` naming both handlers.
//!
//! A path that matches nothing answers 404. A matching path with the wrong method answers 405
//! with `Allow`, which RFC 9110 requires. `HEAD` is answered from the `GET` handler with the body
//! omitted unless a `HEAD` handler exists. `OPTIONS` without a handler answers 204 with `Allow`;
//! a CORS preflight never reaches routing.

pub(crate) mod pattern;

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use http::{HeaderValue, Method};
use ulo::MountedHandler;

use crate::__private::{HandlerFn, HttpHandler};
use crate::cx::PathParams;
use crate::limits::{BodyLimit, Timeout};
use crate::pre_dispatch::{ScopedStage, Stage};
use crate::router::pattern::Pattern;
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
    /// The route's `#[meta(Timeout(..))]`.
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
    /// `OPTIONS` on a matching path with no `OPTIONS` handler: 204 with this `Allow`.
    Options { allow: HeaderValue },
}

/// A route-table failure, naming every handler involved.
#[derive(Debug)]
pub(crate) struct RouteError {
    pub(crate) message: String,
}

impl fmt::Display for RouteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
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
    ) -> Result<Router, Vec<RouteError>> {
        let mut errors = Vec::new();
        let mut groups: Vec<Group> = Vec::new();
        for handler in handlers {
            let who = describe(handler);
            let Some(http) = handler.handler::<HttpHandler>() else {
                errors.push(RouteError { message: format!("{who} is mounted for HTTP without an HTTP handler value") });
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
                    errors.push(RouteError { message: format!("{who}: {error}") });
                    continue;
                }
            };
            let names: Vec<&str> = pattern.param_names().collect();
            for check in &http.path_checks {
                if let Err(reason) = (check.check)(&names) {
                    errors.push(RouteError {
                        message: format!("{who}: `Path<{}>` does not fit the route `{pattern}`: {reason}", check.type_name),
                    });
                }
            }
            let route = if mount.is_empty() { Arc::clone(&pattern.raw) } else { Arc::from(Pattern::join(mount, &pattern.raw)) };
            let target = Arc::new(RouteTarget {
                handler: handler.clone(),
                call: Arc::clone(&http.call),
                pattern: Arc::clone(&pattern.raw),
                route,
                body_limit: info.meta::<BodyLimit>().map_or(default_body_limit, |limit| limit.0),
                timeout: info.meta::<Timeout>().map(|timeout| timeout.0),
                stage: stage.scoped_for(&pattern),
            });
            let method = http.method().clone();
            match groups.iter_mut().find(|group| group.pattern.conflicts(&pattern)) {
                Some(group) if group.pattern.raw != pattern.raw => {
                    let (_, _, other) = &group.targets[0];
                    errors.push(RouteError {
                        message: format!(
                            "the routes `{}` of {other} and `{pattern}` of {who} differ only in parameter names; one position takes one name",
                            group.pattern
                        ),
                    });
                }
                Some(group) => match group.targets.iter().find(|(existing, _, _)| *existing == method) {
                    Some((_, _, other)) => errors.push(RouteError { message: format!("{other} and {who} both answer `{method} {pattern}`") }),
                    None => group.targets.push((method, target, who)),
                },
                None => groups.push(Group { pattern, targets: vec![(method, target, who)] }),
            }
        }
        if !errors.is_empty() {
            return Err(errors);
        }
        let mut routes: Vec<RouteEntry> = groups
            .into_iter()
            .map(|group| {
                let methods: Vec<(Method, Arc<RouteTarget>)> = group.targets.into_iter().map(|(method, target, _)| (method, target)).collect();
                let allow = allow_of(&methods);
                RouteEntry { pattern: group.pattern, methods, allow }
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
                return Routed::Options { allow: entry.allow.clone() };
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
    targets: Vec<(Method, Arc<RouteTarget>, String)>,
}

fn describe(handler: &MountedHandler<Http>) -> String {
    format!("`{}::{}`", handler.controller(), handler.name())
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
