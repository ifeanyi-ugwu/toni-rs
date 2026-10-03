//! CORS, following the Fetch specification's CORS protocol (transports DESIGN §3.4).

use std::error::Error;
use std::fmt;
use std::time::Duration;

use http::header::{
    ACCESS_CONTROL_ALLOW_CREDENTIALS, ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS,
    ACCESS_CONTROL_ALLOW_ORIGIN, ACCESS_CONTROL_EXPOSE_HEADERS, ACCESS_CONTROL_MAX_AGE, ACCESS_CONTROL_REQUEST_HEADERS,
    ACCESS_CONTROL_REQUEST_METHOD, ORIGIN, VARY,
};
use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};

use crate::body::HttpBody;
use crate::middleware::{Middleware, Next};
use crate::request::Request;
use crate::response::Response;

/// A pre-dispatch middleware answering CORS: `m.meta::<PreDispatch>().apply_value(Cors::new()
/// .allow_origin("https://app.example").allow_credentials(true))`.
///
/// A preflight, `OPTIONS` with `Access-Control-Request-Method`, is answered with 204 and the
/// `Access-Control-Allow-*` headers without reaching routing. Every response it passes gets
/// `Vary: Origin`. It never sends `*` together with credentials: the specification forbids the
/// combination, so `prepare` refuses a `Cors` allowing any origin with credentials.
#[derive(Clone, Debug, Default)]
pub struct Cors {
    pub(crate) origins: Origins,
    pub(crate) methods: Vec<Method>,
    pub(crate) headers: AllowHeaders,
    pub(crate) expose: Vec<HeaderName>,
    pub(crate) credentials: bool,
    pub(crate) max_age: Option<Duration>,
}

#[derive(Clone, Debug, Default)]
pub(crate) enum Origins {
    /// No cross-origin request is allowed until one is set.
    #[default]
    None,
    List(Vec<String>),
    Any,
}

#[derive(Clone, Debug, Default)]
pub(crate) enum AllowHeaders {
    #[default]
    None,
    List(Vec<HeaderName>),
    /// Reflect `Access-Control-Request-Headers`.
    Any,
}

impl Cors {
    /// Allows nothing until configured.
    pub fn new() -> Self {
        Cors::default()
    }

    /// One allowed origin, echoed back when it matches; repeatable.
    pub fn allow_origin(mut self, origin: impl Into<String>) -> Self {
        match &mut self.origins {
            Origins::List(list) => list.push(origin.into()),
            _ => self.origins = Origins::List(vec![origin.into()]),
        }
        self
    }

    /// Any origin: `*`, refused in `prepare` together with credentials.
    pub fn allow_any_origin(mut self) -> Self {
        self.origins = Origins::Any;
        self
    }

    pub fn allow_methods(mut self, methods: impl IntoIterator<Item = Method>) -> Self {
        self.methods.extend(methods);
        self
    }

    pub fn allow_headers(mut self, headers: impl IntoIterator<Item = HeaderName>) -> Self {
        let list: Vec<HeaderName> = headers.into_iter().collect();
        self.headers = AllowHeaders::List(list);
        self
    }

    /// Reflects the preflight's `Access-Control-Request-Headers`.
    pub fn allow_any_header(mut self) -> Self {
        self.headers = AllowHeaders::Any;
        self
    }

    pub fn expose_headers(mut self, headers: impl IntoIterator<Item = HeaderName>) -> Self {
        self.expose.extend(headers);
        self
    }

    pub fn allow_credentials(mut self, allow: bool) -> Self {
        self.credentials = allow;
        self
    }

    /// How long a preflight's answer may be cached, as `Access-Control-Max-Age`.
    pub fn max_age(mut self, max_age: Duration) -> Self {
        self.max_age = Some(max_age);
        self
    }

    /// The configuration the specification forbids: `*` with credentials.
    pub(crate) fn check(&self) -> Result<(), CorsError> {
        if self.credentials && matches!(self.origins, Origins::Any) {
            return Err(CorsError { reason: "`*` as the allowed origin cannot be combined with credentials" });
        }
        Ok(())
    }
}

impl Cors {
    /// The `Access-Control-Allow-Origin` value for `origin`, or `None` when it is not allowed.
    /// `Any` with credentials answers `None` too: `prepare` refuses that configuration, and a
    /// `Cors` that escaped the check still never sends `*` with credentials.
    fn allowed_origin(&self, origin: &HeaderValue) -> Option<HeaderValue> {
        match &self.origins {
            Origins::None => None,
            Origins::Any if self.credentials => None,
            Origins::Any => Some(HeaderValue::from_static("*")),
            Origins::List(list) => list.iter().any(|allowed| allowed.as_bytes() == origin.as_bytes()).then(|| origin.clone()),
        }
    }

    fn preflight(&self, req: &Request) -> Response {
        let mut response = Response::new(HttpBody::empty());
        *response.status_mut() = StatusCode::NO_CONTENT;
        let headers = response.headers_mut();
        for name in [ORIGIN, ACCESS_CONTROL_REQUEST_METHOD, ACCESS_CONTROL_REQUEST_HEADERS] {
            vary(headers, name.as_str());
        }
        let Some(allow_origin) = req.headers().get(ORIGIN).and_then(|origin| self.allowed_origin(origin)) else {
            return response;
        };
        headers.insert(ACCESS_CONTROL_ALLOW_ORIGIN, allow_origin);
        if self.credentials {
            headers.insert(ACCESS_CONTROL_ALLOW_CREDENTIALS, HeaderValue::from_static("true"));
        }
        if let Some(methods) = list(self.methods.iter().map(Method::as_str)) {
            headers.insert(ACCESS_CONTROL_ALLOW_METHODS, methods);
        }
        let allow_headers = match &self.headers {
            AllowHeaders::None => None,
            AllowHeaders::List(names) => list(names.iter().map(HeaderName::as_str)),
            AllowHeaders::Any => req.headers().get(ACCESS_CONTROL_REQUEST_HEADERS).cloned(),
        };
        if let Some(allow_headers) = allow_headers {
            headers.insert(ACCESS_CONTROL_ALLOW_HEADERS, allow_headers);
        }
        if let Some(max_age) = self.max_age {
            headers.insert(ACCESS_CONTROL_MAX_AGE, HeaderValue::from(max_age.as_secs()));
        }
        response
    }
}

impl Middleware for Cors {
    async fn handle(&self, req: Request, next: Next<'_>) -> Response {
        let origin = req.headers().get(ORIGIN).cloned();
        let is_preflight =
            req.method() == Method::OPTIONS && origin.is_some() && req.headers().contains_key(ACCESS_CONTROL_REQUEST_METHOD);
        if is_preflight {
            return self.preflight(&req);
        }
        let mut response = next.run(req).await;
        let headers = response.headers_mut();
        // On every response, a request without `Origin` included: a cache must not hand a
        // response without the allow headers to a cross-origin request.
        vary(headers, ORIGIN.as_str());
        let Some(allow_origin) = origin.as_ref().and_then(|origin| self.allowed_origin(origin)) else {
            return response;
        };
        headers.insert(ACCESS_CONTROL_ALLOW_ORIGIN, allow_origin);
        if self.credentials {
            headers.insert(ACCESS_CONTROL_ALLOW_CREDENTIALS, HeaderValue::from_static("true"));
        }
        if let Some(expose) = list(self.expose.iter().map(HeaderName::as_str)) {
            headers.insert(ACCESS_CONTROL_EXPOSE_HEADERS, expose);
        }
        response
    }
}

/// `items` joined as one list-valued header, or `None` when there are none.
fn list<'a>(items: impl Iterator<Item = &'a str>) -> Option<HeaderValue> {
    let joined = items.collect::<Vec<_>>().join(", ");
    if joined.is_empty() {
        return None;
    }
    HeaderValue::from_str(&joined).ok()
}

/// Adds `name` to `Vary` unless a `Vary` value already lists it or is `*`.
fn vary(headers: &mut HeaderMap, name: &str) {
    let listed = headers.get_all(VARY).iter().filter_map(|value| value.to_str().ok()).any(|value| {
        value.split(',').map(str::trim).any(|item| item == "*" || item.eq_ignore_ascii_case(name))
    });
    if !listed {
        if let Ok(value) = HeaderValue::from_str(name) {
            headers.append(VARY, value);
        }
    }
}

/// A `Cors` configuration the Fetch specification forbids, refused in `prepare`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CorsError {
    reason: &'static str,
}

impl fmt::Display for CorsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid CORS configuration: {}", self.reason)
    }
}

impl Error for CorsError {}
