use anyhow::{anyhow, Result};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::watch;

use axum::{
    body::Body,
    extract::{ws::WebSocketUpgrade, Path},
    http::{HeaderMap, HeaderName, HeaderValue, Request, Response, StatusCode},
    routing::{MethodFilter, MethodRouter},
    Router,
};
use futures_util::{FutureExt, SinkExt, StreamExt};
use std::str::FromStr;

use toni::websocket::{WsMessage, WsSink};
use toni::{
    async_trait,
    http_helpers::{PathParams, RequestBody, RequestPart},
    AdapterContext, Body as ToniBody, HttpAdapter, HttpLifecycleHandle, HttpMethod, HttpRequest,
    HttpResponse, MessageCallbackResult, RequestHandler, WebSocketAdapter, WsConnectionCallbacks,
};

use crate::axum_websocket_adapter::{axum_to_ws_message, ws_message_to_axum};
use crate::tokio_sender::TokioSender;

#[derive(Clone)]
pub struct AxumAdapter {
    routes: Vec<(HttpMethod, String, Arc<dyn RequestHandler>)>,
    ws_router: Router,
    ws_ports: HashMap<u16, Router>,
    shutdown_tx: Arc<watch::Sender<bool>>,
}

impl AxumAdapter {
    pub fn new() -> Self {
        let (tx, _) = watch::channel(false);
        Self {
            routes: Vec::new(),
            ws_router: Router::new(),
            ws_ports: HashMap::new(),
            shutdown_tx: Arc::new(tx),
        }
    }
}

impl Default for AxumAdapter {
    fn default() -> Self {
        Self::new()
    }
}

/// Converts toni `:param` path syntax to Axum `{param}` syntax.
fn to_axum_path(path: &str) -> String {
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

fn to_method_filter(method: HttpMethod) -> MethodFilter {
    match method {
        HttpMethod::GET => MethodFilter::GET,
        HttpMethod::POST => MethodFilter::POST,
        HttpMethod::PUT => MethodFilter::PUT,
        HttpMethod::DELETE => MethodFilter::DELETE,
        HttpMethod::PATCH => MethodFilter::PATCH,
        HttpMethod::HEAD => MethodFilter::HEAD,
        HttpMethod::OPTIONS => MethodFilter::OPTIONS,
        HttpMethod::TRACE => MethodFilter::TRACE,
        HttpMethod::CONNECT => MethodFilter::CONNECT,
    }
}

async fn run_ws_connection(
    socket: axum::extract::ws::WebSocket,
    callbacks: Arc<WsConnectionCallbacks>,
    parts: RequestPart,
) {
    let (write, read) = socket.split();
    let (tx, mut rx) = tokio::sync::mpsc::channel::<WsMessage>(32);

    tokio::spawn(async move {
        let mut write = write;
        while let Some(msg) = rx.recv().await {
            if let Ok(axum_msg) = ws_message_to_axum(msg) {
                if write.send(axum_msg).await.is_err() {
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
    let panicked = std::panic::AssertUnwindSafe(async {
        while let Some(result) = read.next().await {
            match result {
                Ok(axum_msg) => match axum_to_ws_message(axum_msg) {
                    Ok(ws_msg) => match callbacks.message(client_id.clone(), ws_msg).await {
                        MessageCallbackResult::Continue => {}
                        MessageCallbackResult::Stop => break,
                        MessageCallbackResult::Stream(stream) => {
                            let sink = sender.clone();
                            let handle = tokio::spawn(async move {
                                use futures_util::StreamExt;
                                tokio::pin!(stream);
                                while let Some(msg) = stream.next().await {
                                    let _ = sink.send(msg).await;
                                }
                            });
                            stream_tasks_inner.lock().unwrap().push(handle);
                        }
                    },
                    Err(_) => {}
                },
                Err(_) => break,
            }
        }
    })
    .catch_unwind()
    .await
    .is_err();

    for handle in stream_tasks.lock().unwrap().drain(..) {
        handle.abort();
    }

    if panicked {
        tracing::error!(client_id = %client_id, "WebSocket handler panicked; closing connection");
    }
    tracing::debug!(client_id = %client_id, "WebSocket connection closed");
    callbacks.disconnect(client_id).await;
}

fn ws_route(callbacks: Arc<WsConnectionCallbacks>) -> axum::routing::MethodRouter {
    axum::routing::get(move |ws: WebSocketUpgrade, req: Request<Body>| {
        let callbacks = callbacks.clone();
        async move {
            let (parts, _body) = req.into_parts();
            let requested_protocol: Option<String> = parts
                .headers
                .get("sec-websocket-protocol")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_owned());
            let ws = match requested_protocol {
                Some(proto) => ws.protocols([proto]),
                None => ws,
            };
            ws.on_upgrade(move |socket| run_ws_connection(socket, callbacks, parts))
        }
    })
}

impl AxumAdapter {
    async fn adapt_request(request: Request<Body>) -> Result<HttpRequest> {
        use http_body_util::BodyExt;

        let (parts, body) = request.into_parts();
        let box_body = body
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
            .boxed_unsync();
        Ok(HttpRequest::from_parts(
            parts,
            RequestBody::Streaming(box_body),
        ))
    }

    async fn adapt_response(response: HttpResponse) -> Result<Response<Body>> {
        let status =
            StatusCode::from_u16(response.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);

        let (body, body_content_type) = match response.body {
            Some(toni_body) => {
                let ct = toni_body.content_type().map(|s| s.to_string());
                (Body::new(toni_body.into_box_body()), ct)
            }
            None => (Body::empty(), None),
        };

        let mut headers = HeaderMap::new();
        if let Some(ct) = body_content_type {
            headers.insert(
                HeaderName::from_str("Content-Type")
                    .map_err(|e| anyhow!("Failed to parse header name: {}", e))?,
                HeaderValue::from_str(&ct)
                    .map_err(|e| anyhow!("Failed to parse content-type value: {}", e))?,
            );
        }

        for (k, v) in &response.headers {
            if let Ok(header_name) = HeaderName::from_bytes(k.as_bytes()) {
                if let Ok(header_value) = HeaderValue::from_str(v) {
                    headers.insert(header_name, header_value);
                }
            }
        }

        let mut res = Response::builder()
            .status(status)
            .body(body)
            .map_err(|e| anyhow!("Failed to build response: {}", e))?;

        res.headers_mut().extend(headers);

        Ok(res)
    }
}

#[toni::async_trait]
impl HttpAdapter for AxumAdapter {
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
        self.ws_router = self.ws_router.clone().route(path, ws_route(callbacks));
        Ok(())
    }

    async fn into_lifecycle(
        mut self: Box<Self>,
        port: u16,
        hostname: &str,
        ctx: AdapterContext,
    ) -> Result<HttpLifecycleHandle> {
        let routes = std::mem::take(&mut self.routes);
        let ctx = Arc::new(ctx);

        // Group routes by path: Axum panics if the same path is registered twice.
        let mut by_path: HashMap<String, MethodRouter> = HashMap::new();
        for (method, path, handler) in routes {
            let axum_path = to_axum_path(&path);
            let filter = to_method_filter(method);
            let handler = handler.clone();
            let ctx = ctx.clone();

            let handler_fn = move |Path(params): Path<HashMap<String, String>>,
                                   req: Request<Body>| {
                let handler = handler.clone();
                let ctx = ctx.clone();
                async move {
                    let (mut parts, body) = req.into_parts();
                    if !params.is_empty() {
                        parts.extensions.insert(PathParams(params));
                    }
                    let req = Request::from_parts(parts, body);

                    let http_req = match Self::adapt_request(req).await {
                        Ok(r) => r,
                        Err(e) => {
                            let body = serde_json::json!({
                                "statusCode": 500,
                                "message": e.to_string(),
                                "error": "Internal Server Error"
                            });
                            return Response::builder()
                                .status(StatusCode::INTERNAL_SERVER_ERROR)
                                .header("Content-Type", "application/json")
                                .body(Body::from(body.to_string()))
                                .unwrap();
                        }
                    };

                    let http_res = ctx
                        .execute(http_req, move |req| {
                            let handler = handler.clone();
                            Box::pin(async move { handler.handle(req).await })
                        })
                        .await;

                    Self::adapt_response(http_res).await.unwrap_or_else(|e| {
                        let body = serde_json::json!({
                            "statusCode": 500,
                            "message": e.to_string(),
                            "error": "Internal Server Error"
                        });
                        Response::builder()
                            .status(StatusCode::INTERNAL_SERVER_ERROR)
                            .header("Content-Type", "application/json")
                            .body(Body::from(body.to_string()))
                            .unwrap()
                    })
                }
            };

            let method_router = axum::routing::on(filter, handler_fn);
            let entry = by_path.entry(axum_path).or_insert_with(MethodRouter::new);
            *entry = std::mem::take(entry).merge(method_router);
        }

        let mut http_router = Router::new();
        for (path, method_router) in by_path {
            http_router = http_router.route(&path, method_router);
        }

        let ctx_fallback = ctx.clone();
        let ws_router = std::mem::replace(&mut self.ws_router, Router::new());
        let router = ws_router
            .merge(http_router)
            .fallback(move |req: Request<Body>| {
                let ctx = ctx_fallback.clone();
                async move {
                    let http_req = match Self::adapt_request(req).await {
                        Ok(r) => r,
                        Err(e) => {
                            let body = serde_json::json!({
                                "statusCode": 500,
                                "message": e.to_string(),
                                "error": "Internal Server Error"
                            });
                            return Response::builder()
                                .status(StatusCode::INTERNAL_SERVER_ERROR)
                                .header("Content-Type", "application/json")
                                .body(Body::from(body.to_string()))
                                .unwrap();
                        }
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
                    Self::adapt_response(http_res).await.unwrap_or_else(|e| {
                        let body = serde_json::json!({
                            "statusCode": 500,
                            "message": e.to_string(),
                            "error": "Internal Server Error"
                        });
                        Response::builder()
                            .status(StatusCode::INTERNAL_SERVER_ERROR)
                            .header("Content-Type", "application/json")
                            .body(Body::from(body.to_string()))
                            .unwrap()
                    })
                }
            });

        let mut shutdown_rx = self.shutdown_tx.subscribe();
        let shutdown_tx = self.shutdown_tx.clone();

        let addr = format!("{}:{}", hostname, port);
        let listener = TcpListener::bind(&addr)
            .await
            .map_err(|e| anyhow!("Failed to bind HTTP port {}: {}", addr, e))?;
        let local_addr = listener
            .local_addr()
            .map_err(|e| anyhow!("Failed to get local address: {}", e))?;

        let serve = Box::pin(async move {
            if let Err(e) = axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.wait_for(|v| *v).await;
                })
                .await
            {
                tracing::error!(error = %e, "HTTP server error");
                std::process::exit(1);
            }
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
impl WebSocketAdapter for AxumAdapter {
    fn bind(&mut self, port: u16, path: &str, callbacks: Arc<WsConnectionCallbacks>) -> Result<()> {
        let router = self.ws_ports.entry(port).or_insert_with(Router::new);
        *router = router.clone().route(path, ws_route(callbacks));
        Ok(())
    }

    async fn into_lifecycle_handles(
        mut self: Box<Self>,
        ports: Vec<(u16, String)>,
    ) -> Result<Vec<toni::WsLifecycleHandle>> {
        let mut handles = Vec::with_capacity(ports.len());
        for (port, hostname) in ports {
            let router = match self.ws_ports.remove(&port) {
                Some(r) => r,
                None => continue,
            };
            let addr = format!("{}:{}", hostname, port);
            let mut shutdown_rx = self.shutdown_tx.subscribe();
            let shutdown_tx = self.shutdown_tx.clone();
            let listener = TcpListener::bind(&addr)
                .await
                .map_err(|e| anyhow!("Failed to bind WebSocket port {}: {}", addr, e))?;
            let local_addr = listener
                .local_addr()
                .map_err(|e| anyhow!("Failed to get local address: {}", e))?;
            let serve = Box::pin(async move {
                axum::serve(listener, router)
                    .with_graceful_shutdown(async move {
                        let _ = shutdown_rx.wait_for(|v| *v).await;
                    })
                    .await
                    .ok();
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
