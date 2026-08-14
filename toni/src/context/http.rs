use std::sync::Arc;

use crate::http_helpers::{HttpRequest, HttpResponse, RequestBody, RequestPart, RouteMetadata};
use crate::traits_helpers::validate::Validatable;

use super::{CancellationToken, Extensions, HandlerContext, shared::SharedState};

/// Per-request context for HTTP handlers.
///
/// Owns the request parts, body, and the (eventual) response. Delegates the
/// universal [`HandlerContext`] surface to its inner `SharedState`.
///
/// The body is an `Option` taken exactly once via
/// [`take_request`](Self::take_request); a second call yields an empty body.
pub struct HttpContext {
    pub(crate) shared: SharedState,
    pub(crate) parts: RequestPart,
    pub(crate) body: Option<RequestBody>,
    pub(crate) response: Option<HttpResponse>,
}

impl HttpContext {
    pub fn new(req: HttpRequest, route_metadata: Arc<RouteMetadata>) -> Self {
        let (mut parts, body) = req.into_parts();
        // `ensure`, not `adopt`: the context and the request it hands the
        // handler must address one bag even when nothing installed one upstream.
        let extensions = Extensions::ensure(&mut parts.extensions);
        Self {
            shared: SharedState::with_extensions(Some(route_metadata), extensions),
            parts,
            body: Some(body),
            response: None,
        }
    }

    pub fn from_request(req: impl Into<HttpRequest>) -> Self {
        let req = req.into();
        let (mut parts, body) = req.into_parts();
        let extensions = Extensions::ensure(&mut parts.extensions);
        Self {
            shared: SharedState::with_extensions(Some(Arc::new(RouteMetadata::new())), extensions),
            parts,
            body: Some(body),
            response: None,
        }
    }

    pub fn from_parts(mut parts: RequestPart) -> Self {
        let extensions = Extensions::ensure(&mut parts.extensions);
        Self {
            shared: SharedState::with_extensions(Some(Arc::new(RouteMetadata::new())), extensions),
            parts,
            body: Some(RequestBody::empty()),
            response: None,
        }
    }

    pub fn request(&self) -> &RequestPart {
        &self.parts
    }

    pub fn response(&self) -> Option<&HttpResponse> {
        self.response.as_ref()
    }

    pub fn response_mut(&mut self) -> Option<&mut HttpResponse> {
        self.response.as_mut()
    }

    pub fn set_response(&mut self, response: HttpResponse) {
        self.response = Some(response);
    }

    pub fn take_response(&mut self) -> Option<HttpResponse> {
        self.response.take()
    }

    /// Reconstruct the full `HttpRequest` (parts + body), consuming the body.
    ///
    /// `None` once the body has been taken. A body is single-use — it may be a
    /// stream — so an enhancer that reads it leaves nothing for the handler, and
    /// the `Option` is what makes that visible at the second call site instead of
    /// handing back a silently empty body.
    pub fn take_request(&mut self) -> Option<HttpRequest> {
        self.body
            .take()
            .map(|body| HttpRequest::from_parts(self.parts.clone(), body))
    }

    /// Consume the context and yield the response. Panics if the response
    /// was never set — used by the dispatcher after the handler chain runs,
    /// where one of the chain steps is guaranteed to have set it.
    pub fn into_response(self) -> HttpResponse {
        self.response.expect("HttpContext: response not set")
    }

    pub fn set_dto(&mut self, dto: Box<dyn Validatable>) {
        self.shared.dto = Some(dto);
    }

    pub fn dto(&self) -> Option<&dyn Validatable> {
        self.shared.dto.as_deref()
    }
}

impl HandlerContext for HttpContext {
    fn route_metadata(&self) -> Option<&RouteMetadata> {
        self.shared.route_metadata.as_deref()
    }

    fn extensions(&self) -> &Extensions {
        &self.shared.extensions
    }

    fn cancellation(&self) -> &CancellationToken {
        &self.shared.cancellation
    }

    fn abort(&mut self) {
        self.shared.abort = true;
    }

    fn should_abort(&self) -> bool {
        self.shared.abort
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
        let mut ctx = HttpContext::from_parts(parts);

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
