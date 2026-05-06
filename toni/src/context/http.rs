use std::sync::Arc;

use parking_lot::Mutex;

use crate::http_helpers::{HttpRequest, HttpResponse, RequestBody, RequestPart, RouteMetadata};
use crate::traits_helpers::validate::Validatable;

use super::{CancellationToken, Extensions, HandlerContext, shared::SharedState};

/// Per-request context for HTTP handlers.
///
/// Owns the request parts, body, and the (eventual) response. Delegates the
/// universal [`HandlerContext`] surface to its inner [`SharedState`].
///
/// The body is wrapped in a `Mutex<Option<...>>` so the type is `Sync` even
/// when the underlying body stream is `!Sync`, and so the handler can take
/// ownership exactly once via [`take_request`](Self::take_request).
pub struct HttpContext {
    pub(crate) shared: SharedState,
    pub(crate) parts: RequestPart,
    pub(crate) body: Mutex<Option<RequestBody>>,
    pub(crate) response: Option<HttpResponse>,
}

impl HttpContext {
    pub fn new(req: HttpRequest, route_metadata: Arc<RouteMetadata>) -> Self {
        let (parts, body) = req.into_parts();
        Self {
            shared: SharedState::new(Some(route_metadata)),
            parts,
            body: Mutex::new(Some(body)),
            response: None,
        }
    }

    pub fn from_request(req: impl Into<HttpRequest>) -> Self {
        let req = req.into();
        let (parts, body) = req.into_parts();
        Self {
            shared: SharedState::new(Some(Arc::new(RouteMetadata::new()))),
            parts,
            body: Mutex::new(Some(body)),
            response: None,
        }
    }

    pub fn from_parts(parts: RequestPart) -> Self {
        Self {
            shared: SharedState::new(Some(Arc::new(RouteMetadata::new()))),
            parts,
            body: Mutex::new(Some(RequestBody::empty())),
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

    /// Reconstruct the full `HttpRequest` (parts + body) and consume the body.
    /// Subsequent calls return an empty body.
    pub fn take_request(&mut self) -> HttpRequest {
        let body = self.body.lock().take().unwrap_or_else(RequestBody::empty);
        HttpRequest::from_parts(self.parts.clone(), body)
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

    fn extensions_mut(&mut self) -> &mut Extensions {
        &mut self.shared.extensions
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
