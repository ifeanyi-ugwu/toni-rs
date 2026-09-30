//! Built-in execution-scoped provider for accessing the current HTTP request.
//!
//! Inject `Request` into a controller to access method, URI, headers,
//! path/query params, and typed extensions set by middleware — without taking
//! the raw `HttpRequest` as a handler argument every time.
//!
//! # Example
//!
//! ```rust
//! use ulo::http::{Body, Request};
//! use ulo::{controller, get, routes};
//!
//! #[controller("/users")]
//! pub struct UserController {
//!     #[inject]
//!     request: Request,
//! }
//!
//! #[routes]
//! impl UserController {
//!     #[get("/me")]
//!     fn get_current_user(&self) -> Body {
//!         let method = self.request.method();
//!         let uri = self.request.uri();
//!         Body::text(format!("Method: {}, URI: {}", method, uri))
//!     }
//! }
//! ```

use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;

use crate::FxHashMap;
use crate::async_trait;
use crate::context::ExecutionContext;
use crate::di::Execution;
use crate::di::ProviderScope;
use crate::error::{BuildResult, ResolutionError};
use crate::extract::FromContext;
use crate::http::HttpContext;
use crate::http::{PathParams, RequestPart};
use crate::spi::{Provider, ProviderFactory};
/// Built-in execution-scoped provider for accessing HTTP request metadata.
///
/// # Scope
///
/// `Request` is execution-scoped and cannot be injected into singleton providers.
#[derive(Clone)]
pub struct Request {
    inner: Arc<RequestPart>,
    path_params: HashMap<String, String>,
    query_params: HashMap<String, String>,
}

#[async_trait]
impl Provider for Request {
    fn token(&self) -> String {
        crate::di::token_of::<Request>()
    }

    async fn resolve(&self, ctx: Execution) -> Result<Box<dyn Any + Send>, ResolutionError> {
        let http_ctx = match &ctx {
            Execution::Http(http_ctx) => http_ctx,
            Execution::None => {
                return Err(ResolutionError::ExecutionRequired {
                    token: self.token(),
                });
            }
            _ => panic!("Request provider requires an HTTP execution context"),
        };
        let cache = http_ctx.cache();
        let instance = match cache.get::<Request>() {
            Some(cached) => cached,
            None => cache.insert(Arc::new(Request::from_parts(http_ctx.request()))),
        };
        Ok(Box::new((*instance).clone()))
    }

    fn scope(&self) -> ProviderScope {
        ProviderScope::Execution
    }
}

impl Request {
    /// Build one from request parts, for a caller holding them rather than a
    /// context.
    pub fn from_parts(parts: &RequestPart) -> Self {
        let path_params = parts
            .extensions
            .get::<PathParams>()
            .map(|p| p.0.clone())
            .unwrap_or_default();

        let query_params = parts
            .uri
            .query()
            .and_then(|q| serde_urlencoded::from_str(q).ok())
            .unwrap_or_default();

        Self {
            inner: Arc::new(parts.clone()),
            path_params,
            query_params,
        }
    }

    pub fn method(&self) -> &str {
        self.inner.method.as_str()
    }

    pub fn uri(&self) -> &http::Uri {
        &self.inner.uri
    }

    /// Get a header value by name (case-insensitive).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.inner.headers.get(name).and_then(|v| v.to_str().ok())
    }

    pub fn headers(&self) -> &http::HeaderMap {
        &self.inner.headers
    }

    pub fn query_params(&self) -> &HashMap<String, String> {
        &self.query_params
    }

    pub fn path_params(&self) -> &HashMap<String, String> {
        &self.path_params
    }

    pub fn extensions(&self) -> &http::Extensions {
        &self.inner.extensions
    }

    pub fn inner(&self) -> &RequestPart {
        &self.inner
    }
}

impl FromContext<HttpContext> for Request {
    type Error = std::convert::Infallible;

    async fn extract(ctx: &HttpContext) -> Result<Self, Self::Error> {
        Ok(Self::from_parts(ctx.request()))
    }
}

pub struct RequestFactory;

#[async_trait]
impl ProviderFactory for RequestFactory {
    fn token(&self) -> String {
        crate::di::token_of::<Request>()
    }

    async fn build(
        &self,
        _deps: FxHashMap<String, crate::spi::Registration>,
    ) -> BuildResult<crate::spi::Registration> {
        let (parts, ()) = http::Request::builder().body(()).unwrap().into_parts();
        let provider = Request::from_parts(&parts);
        Ok(crate::spi::Registration::new(
            Arc::new(provider) as Arc<dyn Provider>,
            vec![],
        ))
    }
}
