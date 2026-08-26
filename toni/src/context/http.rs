use std::sync::Arc;

use parking_lot::Mutex;

use crate::http_helpers::{HttpRequest, RequestBody, RequestPart};

use super::{CancellationToken, Extensions, HandlerContext, Metadata, shared::SharedState};

/// The execution context for one HTTP request.
///
/// A cheap-clone handle: cloning shares the execution rather than copying it, so
/// a streaming response body can hold one and keep reading the bag after the
/// handler has returned.
///
/// The request parts are readable any number of times. The body is not — it may
/// be a stream, so [`take_request`](Self::take_request) yields it exactly once
/// and `None` after that.
///
/// Answering is not done here. A handler returns its response, and an enhancer
/// that wants to answer without reaching the handler returns one too — see
/// [`Interceptor`](crate::traits_helpers::Interceptor).
#[derive(Clone)]
pub struct HttpContext {
    inner: Arc<HttpInner>,
}

struct HttpInner {
    shared: SharedState,
    parts: RequestPart,
    /// The one slot needing interior mutability: single-use, and the context is
    /// shared, so `take` has to be reachable through `&self`.
    body: Mutex<Option<RequestBody>>,
}

impl HttpContext {
    pub fn new(req: HttpRequest, metadata: Arc<Metadata>) -> Self {
        let (mut parts, body) = req.into_parts();
        // `ensure`, not `adopt`: the context and the request it hands the
        // handler must address one bag even when nothing installed one upstream.
        let extensions = Extensions::ensure(&mut parts.extensions);
        Self {
            inner: Arc::new(HttpInner {
                shared: SharedState::with_extensions(Some(metadata), extensions),
                parts,
                body: Mutex::new(Some(body)),
            }),
        }
    }

    pub fn from_request(req: impl Into<HttpRequest>) -> Self {
        let req = req.into();
        let (mut parts, body) = req.into_parts();
        let extensions = Extensions::ensure(&mut parts.extensions);
        Self {
            inner: Arc::new(HttpInner {
                shared: SharedState::with_extensions(Some(Arc::new(Metadata::new())), extensions),
                parts,
                body: Mutex::new(Some(body)),
            }),
        }
    }

    pub fn from_parts(mut parts: RequestPart) -> Self {
        let extensions = Extensions::ensure(&mut parts.extensions);
        Self {
            inner: Arc::new(HttpInner {
                shared: SharedState::with_extensions(Some(Arc::new(Metadata::new())), extensions),
                parts,
                body: Mutex::new(Some(RequestBody::empty())),
            }),
        }
    }

    pub fn request(&self) -> &RequestPart {
        &self.inner.parts
    }

    /// Reconstruct the full `HttpRequest` (parts + body), consuming the body.
    ///
    /// `None` once the body has been taken. A body is single-use — it may be a
    /// stream — so an enhancer that reads it leaves nothing for the handler, and
    /// the `Option` is what makes that visible at the second call site instead of
    /// handing back a silently empty body.
    pub fn take_request(&self) -> Option<HttpRequest> {
        self.inner
            .body
            .lock()
            .take()
            .map(|body| HttpRequest::from_parts(self.inner.parts.clone(), body))
    }
}

impl HandlerContext for HttpContext {
    fn metadata(&self) -> Option<&Metadata> {
        self.inner.shared.metadata.as_deref()
    }

    fn extensions(&self) -> &Extensions {
        &self.inner.shared.extensions
    }

    fn cache(&self) -> &crate::traits_helpers::ExecutionCache {
        &self.inner.shared.cache
    }

    fn cancellation(&self) -> &CancellationToken {
        &self.inner.shared.cancellation
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::HandlerContext;

    #[derive(Clone, PartialEq, Debug)]
    struct Principal(&'static str);

    /// A context built without the adapter seam still puts the request and the
    /// context on one bag, so an enhancer's write survives the handoff that
    /// `take_request` performs on the way to the handler.
    #[test]
    fn a_write_on_the_context_is_visible_on_the_request_it_yields() {
        let parts = http::Request::builder().body(()).unwrap().into_parts().0;
        let ctx = HttpContext::from_parts(parts);

        ctx.extensions().insert(Principal("alice"));

        let req = ctx.take_request().expect("body present on a fresh context");
        let seen = Extensions::adopt(req.extensions());
        assert_eq!(seen.get::<Principal>(), Some(Principal("alice")));
    }

    /// A bag installed upstream is adopted rather than replaced.
    #[test]
    fn an_installed_bag_survives_context_construction() {
        let mut parts = http::Request::builder().body(()).unwrap().into_parts().0;
        let installed = Extensions::install(&mut parts.extensions);
        installed.insert(Principal("bob"));

        let ctx = HttpContext::from_parts(parts);

        assert_eq!(ctx.extensions().get::<Principal>(), Some(Principal("bob")));
    }
}
