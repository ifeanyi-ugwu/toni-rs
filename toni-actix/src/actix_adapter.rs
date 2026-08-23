use anyhow::{anyhow, Context, Result};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use actix_web::body::BoxBody;
use actix_web::dev::{
    forward_ready, Payload, Service as ActixService, ServiceRequest, ServiceResponse, Transform,
};
use actix_web::{
    web, web::Bytes, App, Error as ActixError, FromRequest, HttpMessage,
    HttpRequest as ActixHttpRequest, HttpResponse as ActixHttpResponse, HttpServer, ResponseError,
};
use futures_util::future::LocalBoxFuture;
use toni::{
    http_helpers::{PathParams, RequestBody},
    AdapterContext, BindTarget, Body as ToniBody, HttpAdapter, HttpLifecycleHandle, HttpMethod,
    HttpRequest, HttpResponse, RequestHandler,
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

/// Rewrites Express-style `:param` segments to actix's `{param}`. Toni's own
/// `{param}` paths mount unchanged; the route macros reject `:param`, so only
/// paths registered through the adapter SPI directly can still carry it.
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

fn json_error_response(status: u16, message: String) -> HttpResponse {
    HttpResponse {
        status,
        headers: vec![],
        body: Some(ToniBody::json(serde_json::json!({
            "statusCode": status,
            "message": message,
            "error": match status {
                404 => "Not Found",
                405 => "Method Not Allowed",
                _ => "Internal Server Error",
            },
        }))),
    }
}

/// Match a toni-form path pattern (`/users/{id}` or `/users/:id`) against a
/// concrete path.
fn path_matches(pattern: &str, path: &str) -> bool {
    let mut pattern_segs = pattern.split('/').filter(|s| !s.is_empty());
    let mut path_segs = path.split('/').filter(|s| !s.is_empty());
    loop {
        match (pattern_segs.next(), path_segs.next()) {
            (None, None) => return true,
            (Some(p), Some(s)) if is_param_segment(p) || p == s => {}
            _ => return false,
        }
    }
}

fn is_param_segment(segment: &str) -> bool {
    segment.starts_with(':') || (segment.starts_with('{') && segment.ends_with('}'))
}

fn bytes_to_payload(bytes: Bytes) -> Payload {
    let stream = futures_util::stream::once(std::future::ready(Ok::<
        _,
        actix_web::error::PayloadError,
    >(bytes)));
    Payload::Stream {
        payload: Box::pin(stream),
    }
}

/// Wraps whatever the router produced back into toni's response type for the
/// chain to observe. Bodies are collected — this adapter buffers in both
/// directions by design.
async fn actix_response_to_toni(res: ActixHttpResponse<BoxBody>) -> HttpResponse {
    let status = res.status().as_u16();
    let headers = res
        .headers()
        .iter()
        .filter_map(|(k, v)| {
            v.to_str()
                .ok()
                .map(|v| (k.as_str().to_owned(), v.to_owned()))
        })
        .collect();
    let bytes = actix_web::body::to_bytes(res.into_body())
        .await
        .unwrap_or_else(|_| Bytes::new());
    HttpResponse {
        status,
        headers,
        body: if bytes.is_empty() {
            None
        } else {
            Some(ToniBody::from(bytes))
        },
    }
}

/// App-level middleware factory: the global chain runs once per request,
/// before actix resolves the route. The request the chain forwards is the one
/// routing matches on, so middleware can rewrite paths, short-circuit (auth,
/// CORS preflight), and observe every response — including 404s and 405s.
struct GlobalChain {
    ctx: Arc<AdapterContext>,
}

impl<S> Transform<S, ServiceRequest> for GlobalChain
where
    S: ActixService<ServiceRequest, Response = ServiceResponse<BoxBody>, Error = ActixError>
        + 'static,
{
    type Response = ServiceResponse<BoxBody>;
    type Error = ActixError;
    type Transform = GlobalChainMiddleware<S>;
    type InitError = ();
    type Future = std::future::Ready<std::result::Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        std::future::ready(Ok(GlobalChainMiddleware {
            service: Rc::new(service),
            ctx: self.ctx.clone(),
        }))
    }
}

struct GlobalChainMiddleware<S> {
    service: Rc<S>,
    ctx: Arc<AdapterContext>,
}

impl<S> ActixService<ServiceRequest> for GlobalChainMiddleware<S>
where
    S: ActixService<ServiceRequest, Response = ServiceResponse<BoxBody>, Error = ActixError>
        + 'static,
{
    type Response = ServiceResponse<BoxBody>;
    type Error = ActixError;
    type Future = LocalBoxFuture<'static, std::result::Result<Self::Response, Self::Error>>;

    forward_ready!(service);

    fn call(&self, mut req: ServiceRequest) -> Self::Future {
        let srv = self.service.clone();
        let ctx = self.ctx.clone();
        Box::pin(async move {
            let mut payload = req.take_payload();
            let bytes = Bytes::from_request(req.request(), &mut payload)
                .await
                .unwrap_or_default();

            // The clone is consumed by adapt_request before the dispatch
            // below runs — head_mut requires the request's inner Rc to be
            // unique, so no clone may live across the join.
            let http_req = match ActixAdapter::adapt_request((req.request().clone(), bytes)).await {
                Ok(r) => r,
                Err(_) => {
                    let (base, _) = req.into_parts();
                    let res = ActixHttpResponse::InternalServerError().finish();
                    return Ok(ServiceResponse::new(base, res));
                }
            };

            // Channel bridge: the chain requires a Send routing closure, but
            // actix's inner service is worker-local (!Send). The closure hands
            // the request to — and awaits the response from — the local
            // dispatch future joined alongside the chain below.
            let (req_tx, req_rx) = tokio::sync::oneshot::channel::<HttpRequest>();
            let (res_tx, res_rx) = tokio::sync::oneshot::channel::<HttpResponse>();

            let chain_fut = ctx.execute(http_req, move |treq| {
                Box::pin(async move {
                    if req_tx.send(treq).is_err() {
                        return json_error_response(500, "dispatch unavailable".into());
                    }
                    res_rx
                        .await
                        .unwrap_or_else(|_| json_error_response(500, "dispatch dropped".into()))
                })
            });

            // Resolves to the request the final ServiceResponse is built
            // from — either un-dispatched (chain short-circuited) or the one
            // that travelled through routing.
            let local_fut = async move {
                let Ok(treq) = req_rx.await else {
                    return Some(req.into_parts().0);
                };
                let (parts, body) = treq.into_parts();

                {
                    let head = req.head_mut();
                    if let Ok(m) = parts.method.as_str().parse() {
                        head.method = m;
                    }
                    if let Ok(u) = parts.uri.to_string().parse::<actix_web::http::Uri>() {
                        head.uri = u;
                    }
                    head.headers.clear();
                    for (k, v) in parts.headers.iter() {
                        if let (Ok(name), Ok(value)) = (
                            actix_web::http::header::HeaderName::from_bytes(k.as_str().as_bytes()),
                            actix_web::http::header::HeaderValue::from_bytes(v.as_bytes()),
                        ) {
                            head.headers.append(name, value);
                        }
                    }
                }
                // Routing matches on `match_info`, built before this
                // middleware ran — refresh it from the rewritten URI.
                let uri = req.head().uri.clone();
                req.match_info_mut().get_mut().update(&uri);

                let bytes = body.collect().await.unwrap_or_default();
                req.set_payload(bytes_to_payload(bytes));

                // No request clone may live across this call — actix's router
                // mutates match_info through Rc::get_mut during matching.
                let (ret_req, toni_res) = match srv.call(req).await {
                    Ok(sr) => {
                        let (r, res) = sr.into_parts();
                        (Some(r), actix_response_to_toni(res).await)
                    }
                    Err(e) => (None, actix_response_to_toni(e.error_response()).await),
                };
                let _ = res_tx.send(toni_res);
                ret_req
            };

            let (http_res, base_req) = futures_util::join!(chain_fut, local_fut);

            // The dispatch consumed the request without returning it — an
            // inner service error. The chain observed the error's response;
            // actix renders the propagated error for the client.
            let Some(base_req) = base_req else {
                return Err(actix_web::error::ErrorInternalServerError(
                    "router dispatch failed",
                ));
            };

            let actix_res = ActixAdapter::adapt_response(http_res)
                .await
                .unwrap_or_else(|_| ActixHttpResponse::InternalServerError().finish());
            Ok(ServiceResponse::new(base_req, actix_res))
        })
    }
}

#[toni::async_trait]
impl HttpAdapter for ActixAdapter {
    fn register_route(
        &mut self,
        method: HttpMethod,
        path: &str,
        handler: Arc<dyn RequestHandler>,
    ) -> Result<()> {
        self.routes.push((method, path.to_owned(), handler));
        Ok(())
    }

    async fn into_lifecycle(
        mut self: Box<Self>,
        target: BindTarget,
        ctx: AdapterContext,
    ) -> Result<HttpLifecycleHandle> {
        let addr = target.to_string();
        // actix-server adopts a listener as-is (its `listen` docs push socket
        // configuration to the caller); mio needs it nonblocking.
        let std_listener = target
            .into_std_listener()
            .with_context(|| format!("Failed to bind to {}", addr))?;
        std_listener.set_nonblocking(true)?;
        let routes = std::mem::take(&mut self.routes);
        let ctx = Arc::new(ctx);
        let route_table: Arc<Vec<(HttpMethod, String)>> = Arc::new(
            routes
                .iter()
                .map(|(method, path, _)| (*method, path.clone()))
                .collect(),
        );

        let bound = HttpServer::new(move || {
            let mut app = App::new();
            for (method, path, handler) in &routes {
                let actix_method = to_actix_method(*method);
                let actix_path = to_actix_path(path);
                let handler = handler.clone();
                app = app.route(
                    &actix_path,
                    web::method(actix_method).to(move |req: ActixHttpRequest, body: Bytes| {
                        let handler = handler.clone();
                        async move {
                            let http_req = match Self::adapt_request((req, body)).await {
                                Ok(r) => r,
                                Err(_) => return ActixHttpResponse::InternalServerError().finish(),
                            };
                            let http_res = handler.handle(http_req).await;
                            Self::adapt_response(http_res).await.unwrap_or_else(|_| {
                                ActixHttpResponse::InternalServerError().finish()
                            })
                        }
                    }),
                );
            }
            // Method-aware fallback: actix tries each resource in turn, so a
            // known path with the wrong method also lands here — answer 405
            // with an Allow header; unknown paths get the 404 shape.
            let route_table = route_table.clone();
            app.default_service(web::to(move |req: ActixHttpRequest| {
                let routes = route_table.clone();
                async move {
                    let method = req.method().as_str().to_uppercase();
                    let path = req.path().to_string();

                    let allowed: Vec<String> = routes
                        .iter()
                        .filter(|(_, pattern)| path_matches(pattern, &path))
                        .map(|(m, _)| format!("{:?}", m))
                        .collect();

                    let http_res = if allowed.is_empty() {
                        json_error_response(404, format!("Cannot {} {}", method, path))
                    } else {
                        let mut res = json_error_response(
                            405,
                            format!("Method {} not allowed for {}", method, path),
                        );
                        res.headers.push(("allow".into(), allowed.join(", ")));
                        res
                    };

                    Self::adapt_response(http_res)
                        .await
                        .unwrap_or_else(|_| ActixHttpResponse::InternalServerError().finish())
                }
            }))
            .wrap(GlobalChain { ctx: ctx.clone() })
        })
        .listen(std_listener)
        .with_context(|| format!("Failed to listen on {}", addr))?;

        let local_addr = bound
            .addrs()
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("No bound address for {}", addr))?;

        let running = bound.run();
        let handle = running.handle();
        let serve = Box::pin(async move {
            if let Err(e) = running.await {
                tracing::error!(error = %e, "Actix server error");
                std::process::exit(1);
            }
        });

        Ok(HttpLifecycleHandle::new(
            local_addr,
            serve,
            move || async move {
                handle.stop(true).await;
                Ok(())
            },
        ))
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
