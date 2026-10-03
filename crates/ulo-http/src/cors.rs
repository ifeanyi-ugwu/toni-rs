//! CORS, following the Fetch specification's CORS protocol (transports DESIGN §3.4).

use std::error::Error;
use std::fmt;
use std::time::Duration;

use http::{HeaderName, Method};

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

impl Middleware for Cors {
    async fn handle(&self, req: Request, next: Next<'_>) -> Response {
        let _ = (req, next);
        todo!("answer a preflight with 204; otherwise run `next` and add the allow, expose and `Vary: Origin` headers")
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
