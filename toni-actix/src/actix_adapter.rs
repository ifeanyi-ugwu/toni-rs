use anyhow::{anyhow, Context, Result};
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

use actix_web::{
    web, web::Bytes, App, HttpRequest as ActixHttpRequest,
    HttpResponse as ActixHttpResponse, HttpServer,
};
use toni::{
    AdapterContext, Body as ToniBody, HttpAdapter, HttpMethod, HttpRequest, HttpResponse,
    RequestHandler, ServerHandle, http_helpers::{PathParams, RequestBody},
};

pub struct ActixAdapter {
    routes: Vec<(HttpMethod, String, Arc<dyn RequestHandler>)>,
}

impl ActixAdapter {
    pub fn new() -> Self {
        Self { routes: Vec::new() }
    }
}

impl Default for ActixAdapter {
    fn default() -> Self {
        Self::new()
    }
}

/// Converts toni `:param` path syntax to Actix `{param}` syntax.
fn to_actix_path(path: &str) -> String {
    if !path.contains(':') {
        return path.to_owned();
    }
    let mut out = String::with_capacity(path.len() + 4);
    let mut chars = path.chars().peekable();
    while let Some(c) = chars.next() {
        if c == ':' && chars.peek().map_or(false, |&n| n != '/') {
            out.push('{');
            for n in chars.by_ref() {
                if n == '/' {
                    out.push('}');
                    out.push('/');
                    break;
                }
                out.push(n);
            }
            if !out.ends_with('}') {
                out.push('}');
            }
        } else {
            out.push(c);
        }
    }
    out
}

impl ActixAdapter {
    async fn adapt_request(request: (ActixHttpRequest, Bytes)) -> Result<HttpRequest> {
        let (req, body) = request;

        let method = req
            .method()
            .as_str()
            .parse::<http::Method>()
            .unwrap_or(http::Method::GET);
        let uri = req
            .uri()
            .to_string()
            .parse::<http::Uri>()
            .unwrap_or_else(|_| http::Uri::default());

        let mut builder = http::Request::builder().method(method).uri(uri);
        for (name, value) in req.headers().iter() {
            if let Ok(val) = http::HeaderValue::from_bytes(value.as_bytes()) {
                if let Ok(key) = http::HeaderName::from_bytes(name.as_str().as_bytes()) {
                    builder = builder.header(key, val);
                }
            }
        }
        let (mut http_parts, _) = builder.body(()).unwrap().into_parts();

        let path_params: HashMap<String, String> = req
            .match_info()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        if !path_params.is_empty() {
            http_parts.extensions.insert(PathParams(path_params));
        }

        Ok(HttpRequest::from_parts(
            http_parts,
            RequestBody::Buffered(web::Bytes::from(body.to_vec())),
        ))
    }

    async fn adapt_response(response: HttpResponse) -> Result<ActixHttpResponse> {
        let status = actix_web::http::StatusCode::from_u16(response.status)
            .unwrap_or(actix_web::http::StatusCode::INTERNAL_SERVER_ERROR);

        let mut builder = ActixHttpResponse::build(status);

        let actix_response = match response.body {
            Some(toni_body) => {
                let ct = toni_body
                    .content_type()
                    .unwrap_or("application/octet-stream")
                    .to_string();
                let bytes = {
                    use http_body_util::BodyExt;
                    toni_body
                        .into_box_body()
                        .collect()
                        .await
                        .map(|c| c.to_bytes())
                        .unwrap_or_default()
                };
                builder.content_type(ct.as_str()).body(bytes.to_vec())
            }
            None => builder.finish(),
        };

        let mut actix_response = actix_response;
        for (key, value) in response.headers {
            actix_response.headers_mut().insert(
                actix_web::http::header::HeaderName::from_bytes(key.as_bytes())
                    .map_err(|e| anyhow!("Failed to parse header name: {}", e))?,
                actix_web::http::header::HeaderValue::from_str(&value)
                    .map_err(|e| anyhow!("Failed to parse header value: {}", e))?,
            );
        }

        Ok(actix_response)
    }
}

#[toni::async_trait]
impl HttpAdapter for ActixAdapter {
    fn bind(&mut self, method: HttpMethod, path: &str, handler: Arc<dyn RequestHandler>) -> Result<()> {
        self.routes.push((method, path.to_owned(), handler));
        Ok(())
    }

    fn listen(
        &mut self,
        port: u16,
        hostname: &str,
        ctx: AdapterContext,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<ServerHandle>> + Send + 'static>> {
        let addr = format!("{}:{}", hostname, port);
        let routes = std::mem::take(&mut self.routes);
        let ctx = Arc::new(ctx);

        // actix binds synchronously inside the future — no async bind needed.
        Box::pin(async move {
        let bound = HttpServer::new(move || {
            let ctx = ctx.clone();
            let mut app = App::new();
            for (method, path, handler) in &routes {
                let actix_method = to_actix_method(*method);
                let actix_path = to_actix_path(path);
                let handler = handler.clone();
                let ctx = ctx.clone();
                app = app.route(
                    &actix_path,
                    web::method(actix_method).to(move |req: ActixHttpRequest, body: Bytes| {
                        let handler = handler.clone();
                        let ctx = ctx.clone();
                        async move {
                            let http_req = match Self::adapt_request((req, body)).await {
                                Ok(r) => r,
                                Err(_) => return ActixHttpResponse::InternalServerError().finish(),
                            };
                            let http_res = ctx
                                .execute(http_req, move |req| {
                                    let handler = handler.clone();
                                    Box::pin(async move { handler.handle(req).await })
                                })
                                .await;
                            Self::adapt_response(http_res).await.unwrap_or_else(|_| {
                                ActixHttpResponse::InternalServerError().finish()
                            })
                        }
                    }),
                );
            }
            let ctx_fallback = ctx.clone();
            app.default_service(web::to(move |req: ActixHttpRequest, body: Bytes| {
                let ctx = ctx_fallback.clone();
                async move {
                    let http_req = match Self::adapt_request((req, body)).await {
                        Ok(r) => r,
                        Err(_) => return ActixHttpResponse::InternalServerError().finish(),
                    };
                    let http_res = ctx
                        .execute(http_req, |req| {
                            Box::pin(async move {
                                let method = req.method().as_str().to_uppercase();
                                let path = req.uri().path().to_string();
                                HttpResponse {
                                    status: 404,
                                    headers: vec![],
                                    body: Some(ToniBody::json(serde_json::json!({
                                        "statusCode": 404,
                                        "message": format!("Cannot {} {}", method, path),
                                        "error": "Not Found"
                                    }))),
                                }
                            })
                        })
                        .await;
                    Self::adapt_response(http_res)
                        .await
                        .unwrap_or_else(|_| ActixHttpResponse::InternalServerError().finish())
                }
            }))
        })
        .bind(&addr)
        .with_context(|| format!("Failed to bind to {}", addr))?;

        let local_addr = bound
            .addrs()
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("No bound address for {}", addr))?;

        let running = bound.run();

        Ok(ServerHandle {
            local_addr,
            serve: Box::pin(async move {
                if let Err(e) = running.await {
                    tracing::error!(error = %e, "Actix server error");
                    std::process::exit(1);
                }
            }),
        })
        }) // end Box::pin(async move {
    }
}

fn to_actix_method(method: HttpMethod) -> actix_web::http::Method {
    match method {
        HttpMethod::GET => actix_web::http::Method::GET,
        HttpMethod::POST => actix_web::http::Method::POST,
        HttpMethod::PUT => actix_web::http::Method::PUT,
        HttpMethod::DELETE => actix_web::http::Method::DELETE,
        HttpMethod::PATCH => actix_web::http::Method::PATCH,
        HttpMethod::HEAD => actix_web::http::Method::HEAD,
        HttpMethod::OPTIONS => actix_web::http::Method::OPTIONS,
        HttpMethod::TRACE => actix_web::http::Method::TRACE,
        HttpMethod::CONNECT => actix_web::http::Method::CONNECT,
    }
}
