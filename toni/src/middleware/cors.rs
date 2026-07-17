use async_trait::async_trait;

use crate::http_helpers::HttpResponse;
use crate::traits_helpers::middleware::{Middleware, MiddlewareResult, NextHandle};

/// Origins allowed to make cross-origin requests.
#[derive(Debug, Clone)]
pub enum AllowedOrigins {
    /// Any origin. Responds with `*` — or echoes the caller's origin when
    /// credentials are allowed, since `*` is invalid with credentials.
    Any,
    /// An explicit allowlist, matched exactly against the `Origin` header
    /// (scheme and port included, e.g. `https://app.example.com`).
    List(Vec<String>),
}

impl AllowedOrigins {
    fn allows(&self, origin: &str) -> bool {
        match self {
            AllowedOrigins::Any => true,
            AllowedOrigins::List(origins) => origins.iter().any(|o| o == origin),
        }
    }
}

/// Configuration for [`CorsMiddleware`].
#[derive(Debug, Clone)]
pub struct CorsOptions {
    pub origins: AllowedOrigins,
    /// Methods advertised in preflight responses
    /// (`Access-Control-Allow-Methods`).
    pub methods: Vec<String>,
    /// Headers advertised in preflight responses. `None` reflects whatever
    /// the preflight asked for in `Access-Control-Request-Headers`.
    pub allowed_headers: Option<Vec<String>>,
    /// Response headers browsers may expose to scripts
    /// (`Access-Control-Expose-Headers`).
    pub exposed_headers: Vec<String>,
    /// Sets `Access-Control-Allow-Credentials: true`. Forces origin echoing —
    /// the spec forbids `*` with credentials.
    pub credentials: bool,
    /// Preflight cache lifetime in seconds (`Access-Control-Max-Age`).
    pub max_age: Option<u64>,
}

impl Default for CorsOptions {
    fn default() -> Self {
        Self {
            origins: AllowedOrigins::Any,
            methods: ["GET", "HEAD", "PUT", "PATCH", "POST", "DELETE"]
                .map(String::from)
                .to_vec(),
            allowed_headers: None,
            exposed_headers: vec![],
            credentials: false,
            max_age: None,
        }
    }
}

/// Cross-origin resource sharing for the global middleware chain.
///
/// Register once on the factory; it works identically on every HTTP adapter
/// because the global chain runs before routing — preflight `OPTIONS`
/// requests to routes without an `OPTIONS` handler are answered here, before
/// the router would reject them:
///
/// ```rust,ignore
/// factory.use_global_middleware(Arc::new(CorsMiddleware::permissive()));
/// // or configured:
/// factory.use_global_middleware(Arc::new(CorsMiddleware::new(CorsOptions {
///     origins: AllowedOrigins::List(vec!["https://app.example.com".into()]),
///     credentials: true,
///     ..CorsOptions::default()
/// })));
/// ```
///
/// Requests without an `Origin` header pass through untouched. Requests from
/// a disallowed origin are forwarded (or, for preflight, answered) without
/// CORS headers — the browser enforces the block, per the fetch spec.
pub struct CorsMiddleware {
    options: CorsOptions,
}

impl CorsMiddleware {
    pub fn new(options: CorsOptions) -> Self {
        Self { options }
    }

    /// Any origin, default methods, reflected request headers, no credentials.
    pub fn permissive() -> Self {
        Self::new(CorsOptions::default())
    }

    fn allow_origin_value(&self, origin: &str) -> String {
        match (&self.options.origins, self.options.credentials) {
            (AllowedOrigins::Any, false) => "*".to_string(),
            _ => origin.to_string(),
        }
    }
}

#[async_trait]
impl Middleware for CorsMiddleware {
    async fn handle(&self, next: NextHandle) -> MiddlewareResult {
        let origin = next
            .request()
            .headers()
            .get("origin")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);

        // Same-origin or non-browser request.
        let Some(origin) = origin else {
            return next.run().await;
        };
        let allowed = self.options.origins.allows(&origin);

        let is_preflight = next.request().method() == http::Method::OPTIONS
            && next
                .request()
                .headers()
                .contains_key("access-control-request-method");

        if is_preflight {
            let requested_headers = next
                .request()
                .headers()
                .get("access-control-request-headers")
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);

            let mut response = HttpResponse::no_content().build();
            if allowed {
                response.headers.push((
                    "access-control-allow-origin".into(),
                    self.allow_origin_value(&origin),
                ));
                response.headers.push((
                    "access-control-allow-methods".into(),
                    self.options.methods.join(", "),
                ));
                let allow_headers = match &self.options.allowed_headers {
                    Some(list) => Some(list.join(", ")),
                    None => requested_headers,
                };
                if let Some(headers) = allow_headers {
                    response
                        .headers
                        .push(("access-control-allow-headers".into(), headers));
                }
                if self.options.credentials {
                    response
                        .headers
                        .push(("access-control-allow-credentials".into(), "true".into()));
                }
                if let Some(max_age) = self.options.max_age {
                    response
                        .headers
                        .push(("access-control-max-age".into(), max_age.to_string()));
                }
            }
            // Caches must not reuse this response across origins or
            // requested-header sets.
            response.headers.push((
                "vary".into(),
                "Origin, Access-Control-Request-Method, Access-Control-Request-Headers".into(),
            ));
            return Ok(response);
        }

        let mut response = next.run().await?;
        if allowed {
            response.headers.push((
                "access-control-allow-origin".into(),
                self.allow_origin_value(&origin),
            ));
            if self.options.credentials {
                response
                    .headers
                    .push(("access-control-allow-credentials".into(), "true".into()));
            }
            if !self.options.exposed_headers.is_empty() {
                response.headers.push((
                    "access-control-expose-headers".into(),
                    self.options.exposed_headers.join(", "),
                ));
            }
        }
        response.headers.push(("vary".into(), "Origin".into()));
        Ok(response)
    }
}
