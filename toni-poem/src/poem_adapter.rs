use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use futures_util::{SinkExt, StreamExt, TryStreamExt};
use http_body_util::BodyExt;
use tokio::sync::watch;

use poem::endpoint::{BoxEndpoint, Endpoint, EndpointExt};
use poem::http::StatusCode;
use poem::listener::{Acceptor, Listener, TcpAcceptor, TcpListener};
use poem::web::websocket::WebSocket;
use poem::{
    Body as PoemBody, FromRequest, IntoResponse, Request as PoemRequest, Response as PoemResponse,
    Route, RouteMethod, Server,
};

use toni::websocket::{WsMessage, WsSink};
use toni::{
    async_trait,
    http_helpers::{PathParams, RequestBody, RequestPart},
    AdapterContext, BindTarget, Body as ToniBody, HttpAdapter, HttpLifecycleHandle, HttpMethod,
    HttpRequest, HttpResponse, MessageCallbackResult, RequestHandler, WebSocketAdapter,
    WsConnectionCallbacks,
};

use crate::poem_websocket_adapter::{poem_to_ws_message, ws_message_to_poem};
use crate::tokio_sender::TokioSender;

#[derive(Clone)]
pub struct PoemAdapter {
    routes: Vec<(HttpMethod, String, Arc<dyn RequestHandler>)>,
    ws_routes: Vec<(String, Arc<WsConnectionCallbacks>)>,
    ws_ports: HashMap<u16, Vec<(String, Arc<WsConnectionCallbacks>)>>,
    shutdown_tx: Arc<watch::Sender<bool>>,
}

impl PoemAdapter {
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

impl Default for PoemAdapter {
    fn default() -> Self {
        Self::new()
    }
}

/// Borrow `http::request::Parts` shape out of a poem `Request` without
/// consuming it. Used by the WebSocket endpoint, which needs the parts for
/// the upgrade closure but also needs `&req` to call
/// `WebSocket::from_request_without_body`.
fn extract_parts(req: &PoemRequest) -> RequestPart {
    let params: HashMap<String, String> = req.path_params().unwrap_or_default();

    let mut builder = http::Request::builder()
        .method(req.method().clone())
        .uri(req.uri().clone())
        .version(req.version());
    if let Some(headers) = builder.headers_mut() {
        *headers = req.headers().clone();
    }
    let req_built = builder.body(()).expect("valid request parts");
    let (mut http_parts, _) = req_built.into_parts();
    http_parts.extensions = req.extensions().clone();
    if !params.is_empty() {
        http_parts.extensions.insert(PathParams(params));
    }
    http_parts
}

/// Consume a poem `Request`, returning toni's parts + body pair. Path params
/// must be read before destructuring; everything else falls out of `into_parts`.
fn split_request(req: PoemRequest) -> (RequestPart, RequestBody) {
    let params: HashMap<String, String> = req.path_params().unwrap_or_default();

    let (parts, body) = req.into_parts();

    let mut builder = http::Request::builder()
        .method(parts.method)
        .uri(parts.uri)
        .version(parts.version);
    if let Some(headers) = builder.headers_mut() {
        *headers = parts.headers;
    }
    let req_built = builder.body(()).expect("valid request parts");
    let (mut http_parts, _) = req_built.into_parts();
    http_parts.extensions = parts.extensions;
    if !params.is_empty() {
        http_parts.extensions.insert(PathParams(params));
    }

    let toni_body = poem_body_to_toni(body);
    (http_parts, toni_body)
}

/// Wrap a poem `Body` (an `http_body_util::BoxBody<Bytes, IoError>`) as toni's
/// `RequestBoxBody` (`UnsyncBoxBody<Bytes, Box<dyn Error + Send + Sync>>`).
fn poem_body_to_toni(body: PoemBody) -> RequestBody {
    let inner: http_body_util::combinators::BoxBody<bytes::Bytes, std::io::Error> = body.into();
    let boxed = inner
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
        .boxed_unsync();
    RequestBody::Streaming(boxed)
}

fn toni_response_to_poem(http_res: HttpResponse) -> PoemResponse {
    let status = StatusCode::from_u16(http_res.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);

    let mut builder = PoemResponse::builder().status(status);

    if let Some(body) = http_res.body.as_ref() {
        if let Some(ct) = body.content_type() {
            builder = builder.content_type(ct);
        }
    }

    for (k, v) in &http_res.headers {
        match (
            http::HeaderName::from_bytes(k.as_bytes()),
            http::HeaderValue::from_str(v),
        ) {
            (Ok(name), Ok(value)) => {
                builder = builder.header(name, value);
            }
            (Err(e), _) => {
                tracing::warn!(header = %k, error = %e, "invalid response header name; dropping");
            }
            (_, Err(e)) => {
                tracing::warn!(header = %k, value = %v, error = %e, "invalid response header value; dropping");
            }
        }
    }

    // toni's response body is `Send + !Sync`; poem's `Body::from(BoxBody)`
    // wants the `Send + Sync` boxed body. Buffered bodies map to `from_bytes`
    // (keeps Content-Length); streaming bodies go through `from_bytes_stream`,
    // which needs only `Send`.
    let body = match http_res.body {
        None => PoemBody::empty(),
        Some(toni_body) if toni_body.is_streaming() => PoemBody::from_bytes_stream(
            toni_body
                .into_box_body()
                .into_data_stream()
                .map_err(std::io::Error::other),
        ),
        Some(toni_body) => PoemBody::from_bytes(toni_body.try_bytes().cloned().unwrap_or_default()),
    };

    builder.body(body)
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

/// Rebuild a poem `Body` from toni's, preserving streaming.
fn toni_body_to_poem(body: RequestBody) -> PoemBody {
    match body {
        RequestBody::Buffered(bytes) => PoemBody::from_bytes(bytes),
        RequestBody::Streaming(stream) => {
            PoemBody::from_bytes_stream(stream.into_data_stream().map_err(std::io::Error::other))
        }
    }
}

/// Wraps whatever the router produced — a toni handler's response, a
/// method-mismatch 405, a WebSocket handshake reply — back into toni's
/// response type for the chain to observe. The body is re-wrapped, not read,
/// so streaming responses flow through untouched.
fn poem_response_to_toni(res: PoemResponse) -> HttpResponse {
    let (parts, body) = res.into_parts();
    let headers = parts
        .headers
        .iter()
        .filter_map(|(k, v)| {
            v.to_str()
                .ok()
                .map(|v| (k.as_str().to_owned(), v.to_owned()))
        })
        .collect();
    let inner: http_body_util::combinators::BoxBody<bytes::Bytes, std::io::Error> = body.into();
    let boxed = inner
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
        .boxed_unsync();
    HttpResponse {
        status: parts.status.as_u16(),
        headers,
        body: Some(ToniBody::from_box_body(boxed)),
    }
}

/// Poem endpoint that bridges a single toni HTTP route.
struct ToniEndpoint {
    handler: Arc<dyn RequestHandler>,
}

impl Endpoint for ToniEndpoint {
    type Output = PoemResponse;

    async fn call(&self, req: PoemRequest) -> poem::Result<Self::Output> {
        let (parts, body) = split_request(req);
        let http_req = HttpRequest::from_parts(parts, body);
        let http_res = self.handler.handle(http_req).await;
        Ok(toni_response_to_poem(http_res))
    }
}

/// Convert toni `{param}` path segments to poem's `:param` syntax. `:param`
/// segments are poem-native and pass through.
fn to_poem_path(path: &str) -> String {
    path.split('/')
        .map(|seg| {
            seg.strip_prefix('{')
                .and_then(|s| s.strip_suffix('}'))
                .map(|name| format!(":{name}"))
                .unwrap_or_else(|| seg.to_string())
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Wraps the finished router: the global middleware chain runs once per
/// request, before route matching. The request the chain forwards is the one
/// the router matches on, so middleware can rewrite paths, short-circuit
/// (auth, CORS preflight), and observe every response the router produces
/// natively — including 404s, 405s, and WebSocket handshakes.
struct GlobalChainEndpoint {
    inner: Arc<BoxEndpoint<'static, PoemResponse>>,
    ctx: Arc<AdapterContext>,
}

impl Endpoint for GlobalChainEndpoint {
    type Output = PoemResponse;

    async fn call(&self, mut req: PoemRequest) -> poem::Result<Self::Output> {
        // Pre-routing: no path params yet; the per-route endpoint reads them
        // after poem matches.
        let body = req.take_body();
        let mut builder = http::Request::builder()
            .method(req.method().clone())
            .uri(req.uri().clone())
            .version(req.version());
        if let Some(headers) = builder.headers_mut() {
            *headers = req.headers().clone();
        }
        let built = builder.body(()).expect("valid request parts");
        let (mut parts, _) = built.into_parts();
        parts.extensions = req.extensions().clone();
        let http_req = HttpRequest::from_parts(parts, poem_body_to_toni(body));

        // The poem request shell keeps state http parts cannot carry — the
        // WebSocket upgrade slot — so the chain's output is written back onto
        // the original request instead of rebuilding one.
        let inner = self.inner.clone();

        let http_res = self
            .ctx
            .execute(http_req, move |treq| {
                Box::pin(async move {
                    let (parts, body) = treq.into_parts();
                    req.set_method(parts.method);
                    *req.uri_mut() = parts.uri;
                    req.set_version(parts.version);
                    *req.headers_mut() = parts.headers;
                    *req.extensions_mut() = parts.extensions;
                    req.set_body(toni_body_to_poem(body));

                    // For the unmatched-path 404 below — name the path the
                    // router actually matched on (post-rewrite).
                    let method = req.method().as_str().to_uppercase();
                    let path = req.uri().path().to_string();

                    match inner.call(req).await {
                        Ok(res) => poem_response_to_toni(res),
                        // No catch-all route hosts the toni 404: a `/*` route
                        // would shadow an exact route at `/` (poem's radix
                        // tree prefers a catch-all child over the node's own
                        // data on an empty remainder). The router's
                        // `NotFoundError` is mapped here instead.
                        Err(err) if err.is::<poem::error::NotFoundError>() => {
                            json_error_response(404, format!("Cannot {} {}", method, path))
                        }
                        Err(err) => poem_response_to_toni(err.into_response()),
                    }
                })
            })
            .await;

        Ok(toni_response_to_poem(http_res))
    }
}

/// Poem endpoint that performs a same-port WebSocket upgrade and pipes the
/// connection through toni's `WsConnectionCallbacks`.
struct ToniWsEndpoint {
    callbacks: Arc<WsConnectionCallbacks>,
}

impl Endpoint for ToniWsEndpoint {
    type Output = PoemResponse;

    async fn call(&self, req: PoemRequest) -> poem::Result<Self::Output> {
        // `extract_parts` borrows the request; `from_request_without_body`
        // also borrows. The body never moves — WebSocket upgrade swaps the
        // connection out from under it via `take_upgrade()`.
        let parts = extract_parts(&req);
        let requested_protocol = parts
            .headers
            .get("sec-websocket-protocol")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_owned());

        let ws = WebSocket::from_request_without_body(&req).await?;
        let ws = match requested_protocol {
            Some(p) => ws.protocols([p]),
            None => ws,
        };

        let callbacks = self.callbacks.clone();
        Ok(ws
            .on_upgrade(move |stream| async move {
                run_ws_connection(stream, callbacks, parts).await;
            })
            .into_response())
    }
}

async fn run_ws_connection(
    stream: poem::web::websocket::WebSocketStream,
    callbacks: Arc<WsConnectionCallbacks>,
    parts: RequestPart,
) {
    let (mut write, mut read) = stream.split();
    let (tx, mut rx) = tokio::sync::mpsc::channel::<WsMessage>(32);

    tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            match ws_message_to_poem(msg) {
                Ok(poem_msg) => {
                    if let Err(e) = write.send(poem_msg).await {
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

    while let Some(result) = read.next().await {
        match result {
            Ok(poem_msg) => match poem_to_ws_message(poem_msg) {
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
                Err(e) => {
                    tracing::debug!(client_id = %client_id, error = %e, "ending read loop");
                    break;
                }
            },
            Err(e) => {
                tracing::debug!(client_id = %client_id, error = %e, "poem WebSocket read error");
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

/// Attach a single (method, endpoint) pair to an existing `RouteMethod`. The
/// builder takes `self`, so we pass it through and return the new value.
fn attach_method(
    rm: RouteMethod,
    method: HttpMethod,
    endpoint: BoxEndpoint<'static, PoemResponse>,
) -> RouteMethod {
    match method {
        HttpMethod::GET => rm.get(endpoint),
        HttpMethod::POST => rm.post(endpoint),
        HttpMethod::PUT => rm.put(endpoint),
        HttpMethod::DELETE => rm.delete(endpoint),
        HttpMethod::PATCH => rm.patch(endpoint),
        HttpMethod::HEAD => rm.head(endpoint),
        HttpMethod::OPTIONS => rm.options(endpoint),
        HttpMethod::TRACE => rm.trace(endpoint),
        HttpMethod::CONNECT => rm.connect(endpoint),
    }
}

/// Group routes by path and merge per-method endpoints into a single `RouteMethod`.
/// Poem panics if `.at` is called twice with the same path, so we accumulate every
/// method-handler pair for a given path into one `RouteMethod` before mounting.
fn build_method_routes(
    routes: Vec<(HttpMethod, String, Arc<dyn RequestHandler>)>,
) -> Vec<(String, RouteMethod)> {
    let mut by_path: HashMap<String, RouteMethod> = HashMap::new();
    for (method, path, handler) in routes {
        let endpoint = ToniEndpoint { handler }.map_to_response().boxed();
        let entry = by_path.entry(path).or_insert_with(RouteMethod::new);
        let current = std::mem::replace(entry, RouteMethod::new());
        *entry = attach_method(current, method, endpoint);
    }
    by_path.into_iter().collect()
}

#[toni::async_trait]
impl HttpAdapter for PoemAdapter {
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
        target: BindTarget,
        ctx: AdapterContext,
    ) -> Result<HttpLifecycleHandle> {
        let routes = std::mem::take(&mut self.routes);
        let ws_routes = std::mem::take(&mut self.ws_routes);
        let mut shutdown_rx = self.shutdown_tx.subscribe();
        let shutdown_tx = self.shutdown_tx.clone();

        let mut route = Route::new();
        for (path, route_method) in build_method_routes(routes) {
            route = route.at(to_poem_path(&path), route_method);
        }
        for (path, callbacks) in ws_routes {
            let ws_endpoint = ToniWsEndpoint { callbacks };
            route = route.at(to_poem_path(&path), ws_endpoint);
        }

        let service = GlobalChainEndpoint {
            inner: Arc::new(route.boxed()),
            ctx: Arc::new(ctx),
        };

        let addr = target.to_string();
        let std_listener = target
            .into_std_listener()
            .map_err(|e| anyhow!("Failed to bind HTTP {}: {}", addr, e))?;
        std_listener.set_nonblocking(true)?;
        let acceptor = TcpAcceptor::from_std(std_listener)
            .map_err(|e| anyhow!("Failed to adopt listener for {}: {}", addr, e))?;
        let local_addr = acceptor
            .local_addr()
            .into_iter()
            .next()
            .and_then(|la| la.0.as_socket_addr().copied())
            .ok_or_else(|| anyhow!("Failed to read local address from acceptor"))?;

        let serve = Box::pin(async move {
            let signal = async move {
                let _ = shutdown_rx.wait_for(|v| *v).await;
            };
            if let Err(e) = Server::new_with_acceptor(acceptor)
                .run_with_graceful_shutdown(service, signal, None)
                .await
            {
                tracing::error!(error = %e, "HTTP server error");
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
impl WebSocketAdapter for PoemAdapter {
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

            let mut route = Route::new();
            for (path, callbacks) in routes {
                let ws_endpoint = ToniWsEndpoint { callbacks };
                route = route.at(path, ws_endpoint);
            }

            let addr = format!("{}:{}", hostname, port);
            let mut shutdown_rx = self.shutdown_tx.subscribe();
            let shutdown_tx = self.shutdown_tx.clone();

            let listener = TcpListener::bind(addr.clone());
            let acceptor = listener
                .into_acceptor()
                .await
                .map_err(|e| anyhow!("Failed to bind WebSocket port {}: {}", addr, e))?;
            let local_addr = acceptor
                .local_addr()
                .into_iter()
                .next()
                .and_then(|la| la.0.as_socket_addr().copied())
                .ok_or_else(|| anyhow!("Failed to read local address from acceptor"))?;

            let serve = Box::pin(async move {
                let signal = async move {
                    let _ = shutdown_rx.wait_for(|v| *v).await;
                };
                if let Err(e) = Server::new_with_acceptor(acceptor)
                    .run_with_graceful_shutdown(route, signal, None)
                    .await
                {
                    tracing::error!(error = %e, "WebSocket server error");
                }
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
