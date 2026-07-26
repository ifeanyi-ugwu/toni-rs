use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use futures_util::{SinkExt, StreamExt};
use http_body_util::BodyExt;
use tokio::sync::watch;

use salvo::conn::{Listener, TcpListener};
use salvo::http::body::ResBody;
use salvo::http::{Request as SalvoRequest, Response as SalvoResponse};
use salvo::websocket::WebSocketUpgrade;
use salvo::Router;
use salvo::{async_trait as salvo_async_trait, Depot, FlowCtrl, Handler, Server};

use toni::websocket::{WsMessage, WsSink};
use toni::{
    async_trait,
    http_helpers::{PathParams, RequestBody, RequestPart},
    AdapterContext, Body as ToniBody, HttpAdapter, HttpLifecycleHandle, HttpMethod, HttpRequest,
    HttpResponse, MessageCallbackResult, RequestHandler, WebSocketAdapter, WsConnectionCallbacks,
};

use crate::salvo_websocket_adapter::{salvo_to_ws_message, ws_message_to_salvo};
use crate::tokio_sender::TokioSender;

#[derive(Clone)]
pub struct SalvoAdapter {
    routes: Vec<(HttpMethod, String, Arc<dyn RequestHandler>)>,
    ws_routes: Vec<(String, Arc<WsConnectionCallbacks>)>,
    ws_ports: HashMap<u16, Vec<(String, Arc<WsConnectionCallbacks>)>>,
    shutdown_tx: Arc<watch::Sender<bool>>,
}

impl SalvoAdapter {
    pub fn new() -> Self {
        let (tx, _) = watch::channel(false);
        Self {
            routes: Vec::new(),
            ws_routes: Vec::new(),
            ws_ports: HashMap::new(),
            shutdown_tx: Arc::new(tx),
        }
    }
}

impl Default for SalvoAdapter {
    fn default() -> Self {
        Self::new()
    }
}

/// Converts toni `:param` path syntax to salvo `{param}` syntax. Same shape as axum.
fn to_salvo_path(path: &str) -> String {
    if !path.contains(':') {
        return path.to_owned();
    }
    let mut out = String::with_capacity(path.len() + 4);
    let mut chars = path.chars().peekable();
    while let Some(c) = chars.next() {
        if c == ':' && chars.peek().is_some_and(|&n| n != '/') {
            out.push('{');
            for n in chars.by_ref() {
                if n == '/' {
                    out.push('}');
                    out.push('/');
                    break;
                }
                out.push(n);
            }
            if !out.ends_with('}') && !out.ends_with('/') {
                out.push('}');
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn attach_method(router: Router, method: HttpMethod, handler: ToniRouteHandler) -> Router {
    match method {
        HttpMethod::GET => router.get(handler),
        HttpMethod::POST => router.post(handler),
        HttpMethod::PUT => router.put(handler),
        HttpMethod::DELETE => router.delete(handler),
        HttpMethod::PATCH => router.patch(handler),
        HttpMethod::HEAD => router.head(handler),
        HttpMethod::OPTIONS => router.options(handler),
        // salvo Router has no `.trace()` / `.connect()`; fall back to `.goal()`
        // which runs the handler regardless of method.
        HttpMethod::TRACE | HttpMethod::CONNECT => router.goal(handler),
    }
}

/// Build an `http::request::Parts` from salvo `Request` accessors.
///
/// Carries salvo's request extensions across so anything stashed by upstream
/// middleware survives the boundary, and adds toni's `PathParams` extension
/// which the `Path<T>` extractor reads.
fn build_request_part(req: &SalvoRequest) -> RequestPart {
    let mut builder = http::Request::builder()
        .method(req.method().clone())
        .uri(req.uri().clone())
        .version(req.version());

    if let Some(headers) = builder.headers_mut() {
        *headers = req.headers().clone();
    }

    let req_built = builder.body(()).expect("valid request parts");
    let (mut parts, _) = req_built.into_parts();
    parts.extensions = req.extensions().clone();

    let params: HashMap<String, String> = req
        .params()
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    if !params.is_empty() {
        parts.extensions.insert(PathParams(params));
    }

    parts
}

/// Hand toni an unbuffered body so extractors can stream when they want to.
/// `BodyStream` and friends consume frames directly; `RequestBody::collect` still
/// works for extractors that need the full payload as `Bytes`.
fn take_streaming_body(req: &mut SalvoRequest) -> RequestBody {
    let body = req.take_body();
    let boxed = body
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
        .boxed_unsync();
    RequestBody::Streaming(boxed)
}

fn write_response(http_res: HttpResponse, res: &mut SalvoResponse) {
    let status = http::StatusCode::from_u16(http_res.status)
        .unwrap_or(http::StatusCode::INTERNAL_SERVER_ERROR);
    res.status_code(status);

    if let Some(body) = http_res.body.as_ref() {
        if let Some(ct) = body.content_type() {
            match http::HeaderValue::from_str(ct) {
                Ok(value) => {
                    res.headers_mut().insert(http::header::CONTENT_TYPE, value);
                }
                Err(e) => {
                    tracing::warn!(content_type = %ct, error = %e, "invalid content-type from handler; dropping");
                }
            }
        }
    }

    for (k, v) in &http_res.headers {
        match (
            http::HeaderName::from_bytes(k.as_bytes()),
            http::HeaderValue::from_str(v),
        ) {
            (Ok(name), Ok(value)) => {
                res.headers_mut().insert(name, value);
            }
            (Err(e), _) => {
                tracing::warn!(header = %k, error = %e, "invalid response header name; dropping");
            }
            (_, Err(e)) => {
                tracing::warn!(header = %k, value = %v, error = %e, "invalid response header value; dropping");
            }
        }
    }

    // toni's response body is `Send + !Sync`, but salvo's `ResBody::Boxed`
    // demands `Send + Sync`. Route buffered bodies through the native `Once`
    // variant (keeps Content-Length) and streaming bodies through `stream`,
    // whose `SyncWrapper` accepts a `Send`-only stream without buffering.
    let res_body: ResBody = match http_res.body {
        None => ResBody::None,
        Some(toni_body) if toni_body.is_streaming() => {
            ResBody::stream(toni_body.into_box_body().into_data_stream())
        }
        Some(toni_body) => ResBody::Once(toni_body.try_bytes().cloned().unwrap_or_default()),
    };
    res.body(res_body);
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

/// Carries the chain's request body through the inner dispatch. Salvo's
/// `ReqBody::Boxed` inner type is crate-private, so a streaming toni body
/// cannot be reconstituted into a salvo request — it rides in a request
/// extension instead (routing only needs method and path), and the route
/// handlers take it from there.
#[derive(Clone)]
struct CarriedBody(Arc<std::sync::Mutex<Option<RequestBody>>>);

/// The chain's body if the outer handler carried one, the native body otherwise
/// (separate-port WebSocket servers dispatch without the chain wrapper).
fn take_request_body(req: &mut SalvoRequest) -> RequestBody {
    if let Some(carried) = req.extensions().get::<CarriedBody>() {
        if let Some(body) = carried.0.lock().unwrap().take() {
            return body;
        }
    }
    take_streaming_body(req)
}

/// Wraps whatever the inner dispatch produced — a toni handler's response,
/// the fallback's 404/405 — back into toni's response type for the chain to
/// observe. The body is re-wrapped, not read, so streaming responses flow
/// through untouched.
fn salvo_response_to_toni(mut res: SalvoResponse) -> HttpResponse {
    let status = res.status_code.map(|s| s.as_u16()).unwrap_or(200);
    let headers = res
        .headers()
        .iter()
        .filter_map(|(k, v)| {
            v.to_str()
                .ok()
                .map(|v| (k.as_str().to_owned(), v.to_owned()))
        })
        .collect();
    let boxed = res
        .take_body()
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
        .boxed_unsync();
    HttpResponse {
        status,
        headers,
        body: Some(ToniBody::from_box_body(boxed)),
    }
}

/// Salvo handler that bridges a single toni HTTP route.
struct ToniRouteHandler {
    handler: Arc<dyn RequestHandler>,
}

#[salvo_async_trait]
impl Handler for ToniRouteHandler {
    async fn handle(
        &self,
        req: &mut SalvoRequest,
        _depot: &mut Depot,
        res: &mut SalvoResponse,
        _ctrl: &mut FlowCtrl,
    ) {
        let parts = build_request_part(req);
        let body = take_request_body(req);
        let http_req = HttpRequest::from_parts(parts, body);
        let http_res = self.handler.handle(http_req).await;
        write_response(http_res, res);
    }
}

/// Catch-all fallback with method-aware status: salvo's router cannot
/// distinguish a method mismatch from an unmatched path (method and path are
/// both opaque filters), so the fallback checks the route table — a known
/// path with a different method answers 405 with an Allow header, everything
/// else 404.
struct ToniFallbackHandler {
    routes: Arc<Vec<(HttpMethod, String)>>,
}

/// Match a toni-form path pattern (`/users/:id`) against a concrete path.
fn path_matches(pattern: &str, path: &str) -> bool {
    let mut pattern_segs = pattern.split('/').filter(|s| !s.is_empty());
    let mut path_segs = path.split('/').filter(|s| !s.is_empty());
    loop {
        match (pattern_segs.next(), path_segs.next()) {
            (None, None) => return true,
            (Some(p), Some(s)) if p.starts_with(':') || p == s => {}
            _ => return false,
        }
    }
}

#[salvo_async_trait]
impl Handler for ToniFallbackHandler {
    async fn handle(
        &self,
        req: &mut SalvoRequest,
        _depot: &mut Depot,
        res: &mut SalvoResponse,
        _ctrl: &mut FlowCtrl,
    ) {
        let method = req.method().as_str().to_uppercase();
        let path = req.uri().path().to_string();

        let allowed: Vec<String> = self
            .routes
            .iter()
            .filter(|(_, pattern)| path_matches(pattern, &path))
            .map(|(m, _)| format!("{:?}", m))
            .collect();

        let http_res = if allowed.is_empty() {
            json_error_response(404, format!("Cannot {} {}", method, path))
        } else {
            let mut res =
                json_error_response(405, format!("Method {} not allowed for {}", method, path));
            res.headers.push(("allow".into(), allowed.join(", ")));
            res
        };

        write_response(http_res, res);
    }
}

/// Wraps the inner service: the global middleware chain runs once per
/// request, before route matching. The request the chain forwards is the one
/// the router matches on, so middleware can rewrite paths, short-circuit
/// (auth, CORS preflight), and observe every response the inner dispatch
/// produces — including 404s, 405s, and WebSocket handshakes.
///
/// Served as the goal of a catch-all router; the real router lives inside
/// `inner` and is driven through salvo's public `hyper_handler` entry.
struct GlobalChainHandler {
    inner: Arc<salvo::Service>,
    ctx: Arc<AdapterContext>,
}

#[salvo_async_trait]
impl Handler for GlobalChainHandler {
    async fn handle(
        &self,
        req: &mut SalvoRequest,
        _depot: &mut Depot,
        res: &mut SalvoResponse,
        _ctrl: &mut FlowCtrl,
    ) {
        // The owned request keeps state http parts cannot carry (the
        // WebSocket upgrade, connection addresses, scheme); the chain's
        // output is written back onto it before the inner dispatch.
        let mut owned = std::mem::take(req);
        let parts = build_request_part(&owned);
        let body = take_streaming_body(&mut owned);
        let http_req = HttpRequest::from_parts(parts, body);

        let inner = self.inner.clone();

        let http_res = self
            .ctx
            .execute(http_req, move |treq| {
                Box::pin(async move {
                    let mut sreq = owned;
                    let (parts, body) = treq.into_parts();
                    *sreq.method_mut() = parts.method;
                    *sreq.uri_mut() = parts.uri;
                    *sreq.headers_mut() = parts.headers;
                    *sreq.extensions_mut() = parts.extensions;
                    sreq.extensions_mut()
                        .insert(CarriedBody(Arc::new(std::sync::Mutex::new(Some(body)))));

                    let handler = inner.hyper_handler(
                        sreq.local_addr().clone(),
                        sreq.remote_addr().clone(),
                        sreq.scheme().clone(),
                        None,
                        None,
                    );
                    salvo_response_to_toni(handler.handle(sreq).await)
                })
            })
            .await;

        write_response(http_res, res);
    }
}

/// Salvo handler that performs a same-port WebSocket upgrade and runs the toni callbacks.
struct ToniWsHandler {
    callbacks: Arc<WsConnectionCallbacks>,
}

#[salvo_async_trait]
impl Handler for ToniWsHandler {
    async fn handle(
        &self,
        req: &mut SalvoRequest,
        _depot: &mut Depot,
        res: &mut SalvoResponse,
        _ctrl: &mut FlowCtrl,
    ) {
        let parts = build_request_part(req);
        let callbacks = self.callbacks.clone();

        // Pick up any client-requested subprotocol so the upgrade response advertises it.
        let requested_protocol = parts
            .headers
            .get("sec-websocket-protocol")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_owned());

        let mut upgrade = WebSocketUpgrade::new();
        let proto_storage;
        if let Some(p) = requested_protocol {
            proto_storage = [p];
            let proto_refs: [&str; 1] = [proto_storage[0].as_str()];
            upgrade = upgrade.protocols(&proto_refs);
        }

        let upgrade_result = upgrade
            .upgrade(req, res, move |ws| async move {
                run_ws_connection(ws, callbacks, parts).await;
            })
            .await;

        if let Err(e) = upgrade_result {
            tracing::debug!("WebSocket upgrade failed: {}", e);
        }
    }
}

async fn run_ws_connection(
    ws: salvo::websocket::WebSocket,
    callbacks: Arc<WsConnectionCallbacks>,
    parts: RequestPart,
) {
    let (write, read) = ws.split();
    let (tx, mut rx) = tokio::sync::mpsc::channel::<WsMessage>(32);

    tokio::spawn(async move {
        let mut write = write;
        while let Some(msg) = rx.recv().await {
            match ws_message_to_salvo(msg) {
                Ok(salvo_msg) => {
                    if let Err(e) = write.send(salvo_msg).await {
                        tracing::debug!(error = %e, "WebSocket write failed; closing write task");
                        break;
                    }
                }
                Err(e) => {
                    tracing::debug!(error = %e, "skipping outbound message: convert failed");
                }
            }
        }
    });

    let sender: Arc<dyn WsSink> = Arc::new(TokioSender::new(tx));

    let client_id = match callbacks.connect(parts, sender.clone()).await {
        Ok(id) => id,
        Err(e) => {
            tracing::debug!(error = %e, "WebSocket connect rejected by guard or handler");
            return;
        }
    };

    tracing::debug!(client_id = %client_id, "WebSocket connection established");

    let stream_tasks: Arc<std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let stream_tasks_inner = stream_tasks.clone();

    let mut read = read;
    while let Some(result) = read.next().await {
        match result {
            Ok(salvo_msg) => match salvo_to_ws_message(salvo_msg) {
                Ok(ws_msg) => match callbacks.message(client_id.clone(), ws_msg).await {
                    MessageCallbackResult::Continue => {}
                    MessageCallbackResult::Stop => break,
                    MessageCallbackResult::Stream(stream) => {
                        let sink = sender.clone();
                        let handle = tokio::spawn(async move {
                            tokio::pin!(stream);
                            while let Some(msg) = stream.next().await {
                                if let Err(e) = sink.send(msg).await {
                                    tracing::debug!(
                                        error = %e,
                                        "stream task: sink closed; ending stream"
                                    );
                                    break;
                                }
                            }
                        });
                        stream_tasks_inner.lock().unwrap().push(handle);
                    }
                },
                // Close frames return ConnectionClosed — log at debug, not warn.
                Err(e) => {
                    tracing::debug!(client_id = %client_id, error = %e, "ending read loop");
                    break;
                }
            },
            Err(e) => {
                tracing::debug!(client_id = %client_id, error = %e, "salvo WebSocket read error");
                break;
            }
        }
    }

    for handle in stream_tasks.lock().unwrap().drain(..) {
        handle.abort();
    }

    tracing::debug!(client_id = %client_id, "WebSocket connection closed");
    callbacks.disconnect(client_id).await;
}

#[toni::async_trait]
impl HttpAdapter for SalvoAdapter {
    fn register_route(
        &mut self,
        method: HttpMethod,
        path: &str,
        handler: Arc<dyn RequestHandler>,
    ) -> Result<()> {
        self.routes.push((method, path.to_owned(), handler));
        Ok(())
    }

    fn register_ws_route(
        &mut self,
        path: &str,
        callbacks: Arc<WsConnectionCallbacks>,
    ) -> Result<()> {
        self.ws_routes.push((path.to_owned(), callbacks));
        Ok(())
    }

    async fn into_lifecycle(
        mut self: Box<Self>,
        port: u16,
        hostname: &str,
        ctx: AdapterContext,
    ) -> Result<HttpLifecycleHandle> {
        let routes = std::mem::take(&mut self.routes);
        let ws_routes = std::mem::take(&mut self.ws_routes);
        let mut shutdown_rx = self.shutdown_tx.subscribe();
        let shutdown_tx = self.shutdown_tx.clone();

        let route_table: Arc<Vec<(HttpMethod, String)>> = Arc::new(
            routes
                .iter()
                .map(|(method, path, _)| (*method, path.clone()))
                .collect(),
        );

        let mut router = Router::new();
        for (method, path, handler) in routes {
            let salvo_path = to_salvo_path(&path);
            let toni_handler = ToniRouteHandler { handler };
            let sub = attach_method(Router::with_path(salvo_path), method, toni_handler);
            router = router.push(sub);
        }
        for (path, callbacks) in ws_routes {
            let salvo_path = to_salvo_path(&path);
            let ws_handler = ToniWsHandler { callbacks };
            router = router.push(Router::with_path(salvo_path).goal(ws_handler));
        }
        let fallback = ToniFallbackHandler {
            routes: route_table,
        };
        router = router.push(Router::with_path("{**rest}").goal(fallback));

        let chain = GlobalChainHandler {
            inner: Arc::new(salvo::Service::new(router)),
            ctx: Arc::new(ctx),
        };
        let router = Router::with_path("{**rest}").goal(chain);

        let addr = format!("{}:{}", hostname, port);
        let acceptor = TcpListener::new(addr.clone())
            .try_bind()
            .await
            .map_err(|e| anyhow!("Failed to bind HTTP port {}: {}", addr, e))?;
        let local_addr = acceptor
            .local_addr()
            .map_err(|e| anyhow!("Failed to get local address: {}", e))?;

        let server = Server::new(acceptor);
        let server_handle = server.handle();

        tokio::spawn(async move {
            let _ = shutdown_rx.wait_for(|v| *v).await;
            server_handle.stop_graceful(None);
        });

        let serve = Box::pin(async move {
            server.serve(router).await;
        });

        Ok(HttpLifecycleHandle::new(
            local_addr,
            serve,
            move || async move {
                let _ = shutdown_tx.send(true);
                Ok(())
            },
        ))
    }
}

#[async_trait]
impl WebSocketAdapter for SalvoAdapter {
    fn register_gateway(
        &mut self,
        port: u16,
        path: &str,
        callbacks: Arc<WsConnectionCallbacks>,
    ) -> Result<()> {
        self.ws_ports
            .entry(port)
            .or_default()
            .push((path.to_owned(), callbacks));
        Ok(())
    }

    async fn into_lifecycle_handles(
        mut self: Box<Self>,
        ports: Vec<(u16, String)>,
    ) -> Result<Vec<toni::WsLifecycleHandle>> {
        let mut handles = Vec::with_capacity(ports.len());
        for (port, hostname) in ports {
            let routes = match self.ws_ports.remove(&port) {
                Some(r) => r,
                None => continue,
            };

            let mut router = Router::new();
            for (path, callbacks) in routes {
                let salvo_path = to_salvo_path(&path);
                let ws_handler = ToniWsHandler { callbacks };
                router = router.push(Router::with_path(salvo_path).goal(ws_handler));
            }

            let addr = format!("{}:{}", hostname, port);
            let mut shutdown_rx = self.shutdown_tx.subscribe();
            let shutdown_tx = self.shutdown_tx.clone();

            let acceptor = TcpListener::new(addr.clone())
                .try_bind()
                .await
                .map_err(|e| anyhow!("Failed to bind WebSocket port {}: {}", addr, e))?;
            let local_addr = acceptor
                .local_addr()
                .map_err(|e| anyhow!("Failed to get local address: {}", e))?;

            let server = Server::new(acceptor);
            let server_handle = server.handle();

            tokio::spawn(async move {
                let _ = shutdown_rx.wait_for(|v| *v).await;
                server_handle.stop_graceful(None);
            });

            let serve = Box::pin(async move {
                server.serve(router).await;
            });

            handles.push(toni::WsLifecycleHandle::new(
                local_addr,
                serve,
                move || async move {
                    let _ = shutdown_tx.send(true);
                    Ok(())
                },
            ));
        }
        Ok(handles)
    }
}

#[cfg(test)]
mod tests {
    use super::to_salvo_path;

    #[test]
    fn no_params_returns_input_unchanged() {
        assert_eq!(to_salvo_path("/"), "/");
        assert_eq!(to_salvo_path("/users"), "/users");
        assert_eq!(to_salvo_path("/api/v1/users"), "/api/v1/users");
    }

    #[test]
    fn single_param_in_middle() {
        assert_eq!(to_salvo_path("/users/:id/posts"), "/users/{id}/posts");
    }

    #[test]
    fn single_param_at_end() {
        assert_eq!(to_salvo_path("/users/:id"), "/users/{id}");
    }

    #[test]
    fn single_param_at_root() {
        assert_eq!(to_salvo_path("/:name"), "/{name}");
    }

    #[test]
    fn back_to_back_params_separated_by_slash() {
        assert_eq!(to_salvo_path("/:a/:b"), "/{a}/{b}");
        assert_eq!(
            to_salvo_path("/users/:id/comments/:cid"),
            "/users/{id}/comments/{cid}"
        );
    }

    #[test]
    fn trailing_slash_after_param_preserved() {
        assert_eq!(to_salvo_path("/users/:id/"), "/users/{id}/");
    }
}
