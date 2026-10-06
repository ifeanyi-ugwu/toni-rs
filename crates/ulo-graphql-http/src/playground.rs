//! The playground: the page `Engine::playground_html` answers, on a GET with `Accept: text/html`
//! and no `query` parameter, under `GraphqlConfig::playground`.

use http::header::{CONTENT_TYPE, HeaderMap, HeaderValue};
use ulo_graphql::Engine;
use ulo_http::{HttpBody, Response};

use crate::controller::{EndpointSettings, accept_ranges};

/// Whether the client names `text/html` itself with a nonzero quality. A wildcard does not count:
/// `curl` sends `*/*`, and a browser loading the page names `text/html`.
pub(crate) fn accepts_html(headers: &HeaderMap) -> bool {
    accept_ranges(headers).iter().any(|range| range.kind == "text" && range.subtype == "html" && range.q > 0)
}

/// The engine's page for this endpoint, `None` when the engine has none. The paths it is given are
/// the ones a browser reaches: `mount_prefix` is the embedding host's prefix, empty on a backend.
pub(crate) fn page<Q>(engine: &dyn Engine, settings: &EndpointSettings<Q>, mount_prefix: &str) -> Option<Response> {
    let endpoint = format!("{mount_prefix}{}", settings.path);
    let subscriptions = settings.subscriptions.as_ref().map(|path| format!("{mount_prefix}{path}"));
    let html = engine.playground_html(&endpoint, subscriptions.as_deref())?;
    let mut response = Response::new(HttpBody::from_bytes(html));
    response.headers_mut().insert(CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
    Some(response)
}
