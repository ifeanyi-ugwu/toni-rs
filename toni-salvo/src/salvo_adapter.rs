use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use futures_util::{SinkExt, StreamExt};
use http_body_util::BodyExt;
use tokio::sync::watch;

use salvo::Router;
use salvo::conn::{Listener, TcpListener};
use salvo::http::{Request as SalvoRequest, Response as SalvoResponse};
use salvo::http::body::ResBody;
use salvo::websocket::WebSocketUpgrade;
use salvo::{Depot, FlowCtrl, Handler, Server, async_trait as salvo_async_trait};

use toni::websocket::{WsMessage, WsSink};
use toni::{
    AdapterContext, Body as ToniBody, HttpAdapter, HttpMethod, HttpRequest, HttpResponse,
    MessageCallbackResult, RequestHandler, ServerHandle, WebSocketAdapter, WsConnectionCallbacks,
    async_trait,
    http_helpers::{PathParams, RequestBody, RequestPart},
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
            if let Ok(value) = http::HeaderValue::from_str(ct) {
                res.headers_mut().insert(http::header::CONTENT_TYPE, value);
            }
        }
    }

    for (k, v) in &http_res.headers {
        if let (Ok(name), Ok(value)) = (
            http::HeaderName::from_bytes(k.as_bytes()),
            http::HeaderValue::from_str(v),
        ) {
            res.headers_mut().insert(name, value);
        }
    }

    let res_body: ResBody = match http_res.body {
        Some(toni_body) => ResBody::Boxed(Box::pin(toni_body.into_box_body())),
        None => ResBody::None,
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
            "error": if status == 404 { "Not Found" } else { "Internal Server Error" },
        }))),
    }
}

/// Salvo handler that bridges a single toni HTTP route through the adapter context.
struct ToniRouteHandler {
    handler: Arc<dyn RequestHandler>,
    ctx: Arc<AdapterContext>,
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
        let body = take_streaming_body(req);
        let http_req = HttpRequest::from_parts(parts, body);

        let handler = self.handler.clone();
        let http_res = self
            .ctx
            .execute(http_req, move |req| {
                let handler = handler.clone();
                Box::pin(async move { handler.handle(req).await })
            })
            .await;

        write_response(http_res, res);
    }
}

/// Salvo handler for the catch-all fallback — runs the global chain and emits a 404.
struct ToniFallbackHandler {
    ctx: Arc<AdapterContext>,
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
        let parts = build_request_part(req);
        let body = take_streaming_body(req);
        let http_req = HttpRequest::from_parts(parts, body);

        let http_res = self
            .ctx
            .execute(http_req, |req| {
                Box::pin(async move {
                    let method = req.method().as_str().to_uppercase();
                    let path = req.uri().path().to_string();
                    json_error_response(404, format!("Cannot {} {}", method, path))
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
            if let Ok(salvo_msg) = ws_message_to_salvo(msg) {
                if write.send(salvo_msg).await.is_err() {
                    break;
                }
            }
        }
    });

    let sender: Arc<dyn WsSink> = Arc::new(TokioSender::new(tx));

    let client_id = match callbacks.connect(parts, sender.clone()).await {
        Ok(id) => id,
        Err(_) => return,
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
                                let _ = sink.send(msg).await;
                            }
                        });
                        stream_tasks_inner.lock().unwrap().push(handle);
                    }
                },
                Err(_) => break,
            },
            Err(_) => break,
        }
    }

    for handle in stream_tasks.lock().unwrap().drain(..) {
        handle.abort();
    }

    tracing::debug!(client_id = %client_id, "WebSocket connection closed");
    callbacks.disconnect(client_id).await;
}

impl HttpAdapter for SalvoAdapter {
    fn bind(
        &mut self,
        method: HttpMethod,
        path: &str,
        handler: Arc<dyn RequestHandler>,
    ) -> Result<()> {
        self.routes.push((method, path.to_owned(), handler));
        Ok(())
    }

    fn bind_ws(&mut self, path: &str, callbacks: Arc<WsConnectionCallbacks>) -> Result<()> {
        self.ws_routes.push((path.to_owned(), callbacks));
        Ok(())
    }

    fn listen(
        &mut self,
        port: u16,
        hostname: &str,
        ctx: AdapterContext,
    ) -> Pin<Box<dyn Future<Output = Result<ServerHandle>> + Send + 'static>> {
        let routes = std::mem::take(&mut self.routes);
        let ws_routes = std::mem::take(&mut self.ws_routes);
        let ctx = Arc::new(ctx);
        let mut shutdown_rx = self.shutdown_tx.subscribe();

        let mut router = Router::new();
        for (method, path, handler) in routes {
            let salvo_path = to_salvo_path(&path);
            let toni_handler = ToniRouteHandler {
                handler,
                ctx: ctx.clone(),
            };
            let sub = attach_method(Router::with_path(salvo_path), method, toni_handler);
            router = router.push(sub);
        }
        for (path, callbacks) in ws_routes {
            let salvo_path = to_salvo_path(&path);
            let ws_handler = ToniWsHandler { callbacks };
            router = router.push(Router::with_path(salvo_path).goal(ws_handler));
        }
        let fallback = ToniFallbackHandler { ctx: ctx.clone() };
        router = router.push(Router::with_path("{**rest}").goal(fallback));

        let addr = format!("{}:{}", hostname, port);
        Box::pin(async move {
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

            Ok(ServerHandle {
                local_addr,
                serve: Box::pin(async move {
                    server.serve(router).await;
                }),
            })
        })
    }

    fn close(&mut self) -> impl Future<Output = Result<()>> + Send {
        let tx = self.shutdown_tx.clone();
        async move {
            let _ = tx.send(true);
            Ok(())
        }
    }
}

#[async_trait]
impl WebSocketAdapter for SalvoAdapter {
    fn bind(
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

    fn listen(
        &mut self,
        port: u16,
        hostname: &str,
    ) -> Pin<Box<dyn Future<Output = Result<ServerHandle>> + Send + 'static>> {
        let routes = match self.ws_ports.remove(&port) {
            Some(r) => r,
            None => {
                return Box::pin(async move {
                    Err(anyhow!("No routes registered for WS port {}", port))
                });
            }
        };

        let mut router = Router::new();
        for (path, callbacks) in routes {
            let salvo_path = to_salvo_path(&path);
            let ws_handler = ToniWsHandler { callbacks };
            router = router.push(Router::with_path(salvo_path).goal(ws_handler));
        }

        let addr = format!("{}:{}", hostname, port);
        let mut shutdown_rx = self.shutdown_tx.subscribe();

        Box::pin(async move {
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

            Ok(ServerHandle {
                local_addr,
                serve: Box::pin(async move {
                    server.serve(router).await;
                }),
            })
        })
    }
}
