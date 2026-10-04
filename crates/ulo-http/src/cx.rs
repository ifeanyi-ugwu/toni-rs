use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use http::{HeaderMap, Method, Uri};
use ulo::{AppHandle, ExecutionRef, Ext, Extensions, LookupError, MountedHandler, Timer};

use crate::backend::HttpConfig;
use crate::body::HttpBody;
use crate::request::{ConnInfo, OnUpgrade};
use crate::transport::{Http, RequestHead};

/// The per-request context: `Clone + Send + Sync`, every clone the same request (transports
/// DESIGN §3.1). An `ExecutionRef`, the request head, the matched route and its path parameters,
/// the client address, a take-once body slot and an interior-mutable response-head writer.
///
/// A guard reads and writes through it: `cx.head()`, `cx.extensions().insert(..)`,
/// `cx.exec().handler()`. A handler takes it as a parameter. A streaming reply holds a clone, which
/// keeps the execution's instances and its cancellation signal alive while the stream runs.
#[derive(Clone)]
pub struct HttpCx {
    pub(crate) inner: Arc<CxInner>,
}

pub(crate) struct CxInner {
    pub(crate) exec: ExecutionRef,
    pub(crate) app: AppHandle,
    pub(crate) head: Arc<RequestHead>,
    pub(crate) conn: ConnInfo,
    /// `None` for a request that matched no route, whose context the global error handlers read.
    pub(crate) route: Option<MatchedRoute>,
    pub(crate) body: Mutex<Option<HttpBody>>,
    pub(crate) upgrade: Mutex<Option<OnUpgrade>>,
    /// Merged into the response's headers when it is written, the reply's own winning.
    pub(crate) response_headers: Mutex<HeaderMap>,
    pub(crate) config: Arc<HttpConfig>,
    /// The embedding's normalized `.nested_at` prefix; empty otherwise.
    pub(crate) mount: Arc<str>,
    pub(crate) timer: Arc<dyn Timer>,
}

/// The route a request matched.
pub(crate) struct MatchedRoute {
    pub(crate) handler: MountedHandler<Http>,
    /// The route pattern as written, prefix applied: `/users/{id}`.
    pub(crate) pattern: Arc<str>,
    pub(crate) params: PathParams,
    /// The server's body limit, or the route's `#[meta(BodyLimit(..))]`.
    pub(crate) body_limit: u64,
}

/// A matched route's path parameters, percent-decoded, in the order the pattern names them.
#[derive(Clone, Debug, Default)]
pub struct PathParams {
    pub(crate) pairs: Vec<(Arc<str>, String)>,
}

impl PathParams {
    pub fn get(&self, name: &str) -> Option<&str> {
        self.pairs.iter().find(|(key, _)| &**key == name).map(|(_, value)| value.as_str())
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> + '_ {
        self.pairs.iter().map(|(key, value)| (&**key, value.as_str()))
    }

    pub fn len(&self) -> usize {
        self.pairs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pairs.is_empty()
    }
}

impl HttpCx {
    /// The request head as the pre-dispatch stage left it, a rewritten path included. The seeded
    /// `Dep<RequestHead>` input is the head as the client sent it.
    pub fn head(&self) -> &RequestHead {
        &self.inner.head
    }

    pub fn method(&self) -> &Method {
        self.inner.head.method()
    }

    pub fn uri(&self) -> &Uri {
        self.inner.head.uri()
    }

    pub fn headers(&self) -> &HeaderMap {
        self.inner.head.headers()
    }

    /// The matched route's pattern, the controller's prefix applied: `/users/{id}`. `None` on a
    /// miss. An embedded app nested under a prefix routes on the path without it, so this is the
    /// pattern without the [`mount_prefix`](Self::mount_prefix).
    pub fn route(&self) -> Option<&str> {
        self.inner.route.as_ref().map(|route| &*route.pattern)
    }

    /// Where the host mounted the app: the embedding's `.nested_at` prefix with a leading `/` and
    /// no trailing one, `/api`, and empty for an app that is not nested, on a backend included.
    /// The host strips it before the app sees the request, so a `Location` the client follows is
    /// `format!("{}{path}", cx.mount_prefix())` for an app path starting with `/`, which never
    /// doubles a slash.
    pub fn mount_prefix(&self) -> &str {
        &self.inner.mount
    }

    /// One path parameter of the matched route, percent-decoded.
    pub fn param(&self, name: &str) -> Option<&str> {
        self.inner.route.as_ref()?.params.get(name)
    }

    pub fn params(&self) -> Option<&PathParams> {
        self.inner.route.as_ref().map(|route| &route.params)
    }

    pub fn exec(&self) -> &ExecutionRef {
        &self.inner.exec
    }

    pub fn extensions(&self) -> &Extensions {
        self.inner.exec.extensions()
    }

    /// The extension `T` a guard or middleware wrote, as `Ext<T>` reads it.
    pub fn ext<T: Send + Sync + 'static>(&self) -> Result<Ext<T>, LookupError> {
        self.inner.exec.resolver().ext::<T>()
    }

    /// The request body, once: a second take, or a take after a body extractor, is `None`.
    pub fn take_body(&self) -> Option<HttpBody> {
        self.inner.body.lock().unwrap_or_else(PoisonError::into_inner).take()
    }

    /// The upgrade future, once, for the WebSocket hand-off.
    pub fn take_upgrade(&self) -> Option<OnUpgrade> {
        self.inner.upgrade.lock().unwrap_or_else(PoisonError::into_inner).take()
    }

    /// Headers merged into the response when it is written; the reply's own headers win.
    pub fn response_headers(&self) -> MutexGuard<'_, HeaderMap> {
        self.inner.response_headers.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn client_addr(&self) -> Option<SocketAddr> {
        self.inner.conn.peer
    }

    pub fn conn(&self) -> &ConnInfo {
        &self.inner.conn
    }

    /// The app, for `AppHandle::redact` and lookups outside the execution.
    pub fn app(&self) -> &AppHandle {
        &self.inner.app
    }

    /// The app's `Timer`. A reply that waits on a clock of its own reads this one, as
    /// `Sse::keep_alive` does, so the app's runtime drives it, and a paused test clock with it.
    pub fn timer(&self) -> &Arc<dyn Timer> {
        &self.inner.timer
    }

    pub(crate) fn matched(&self) -> Option<&MatchedRoute> {
        self.inner.route.as_ref()
    }
}

impl AsRef<ExecutionRef> for HttpCx {
    fn as_ref(&self) -> &ExecutionRef {
        &self.inner.exec
    }
}
