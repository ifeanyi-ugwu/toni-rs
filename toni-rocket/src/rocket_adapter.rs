use std::collections::HashMap;
use std::convert::TryFrom;
use std::io::Cursor;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::watch;

use rocket::data::{ByteUnit, Data};
use rocket::fairing::AdHoc;
use rocket::http::{Method as RocketMethod, Status};
use rocket::request::{FromRequest, Request as RocketRequest};
use rocket::response::Response as RocketResponse;
use rocket::route::{Handler, Outcome, Route};
use rocket::Config;
use rocket_ws::WebSocket as RocketWs;

use toni::websocket::{WsMessage, WsSink};
use toni::{
    http_helpers::{PathParams, RequestBody, RequestPart},
    AdapterContext, Body as ToniBody, HttpAdapter, HttpLifecycleHandle, HttpMethod, HttpRequest,
    HttpResponse, MessageCallbackResult, RequestHandler, WsConnectionCallbacks,
};

use crate::rocket_websocket_adapter::{rocket_to_ws_message, ws_message_to_rocket};
use crate::tokio_sender::TokioSender;

/// Default request-body size limit. Rocket requires an explicit limit on
/// `Data::open(limit)` and we have to materialize the body into `Bytes`
/// before handing it to toni — `Data<'r>` can't outlive the request, so the
/// other adapters' streaming wrappers won't work here. 32 MiB matches the
/// "reasonable upload" ceiling most production HTTP frameworks ship with.
const REQUEST_BODY_LIMIT: ByteUnit = ByteUnit::Mebibyte(32);

#[derive(Clone)]
pub struct RocketAdapter {
    routes: Vec<(HttpMethod, String, Arc<dyn RequestHandler>)>,
    ws_routes: Vec<(String, Arc<WsConnectionCallbacks>)>,
    shutdown_tx: Arc<watch::Sender<bool>>,
}

impl RocketAdapter {
    pub fn new() -> Self {
        let (tx, _) = watch::channel(false);
        Self {
            routes: Vec::new(),
            ws_routes: Vec::new(),
            shutdown_tx: Arc::new(tx),
        }
    }
}

impl Default for RocketAdapter {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert toni's `:param` syntax to rocket's `<param>` (and `*tail` to
/// `<tail..>`). Rocket's URI parser would otherwise reject toni's paths.
fn to_rocket_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len() + 4);
    let mut chars = path.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            ':' if chars.peek().is_some_and(|&n| n != '/') => {
                out.push('<');
                while let Some(&n) = chars.peek() {
                    if n == '/' {
                        break;
                    }
                    out.push(n);
                    chars.next();
                }
                out.push('>');
            }
            '*' if chars.peek().is_some_and(|&n| n != '/') => {
                out.push('<');
                while let Some(&n) = chars.peek() {
                    if n == '/' {
                        break;
                    }
                    out.push(n);
                    chars.next();
                }
                out.push_str("..>");
            }
            _ => out.push(c),
        }
    }
    out
}

fn to_rocket_method(method: HttpMethod) -> RocketMethod {
    match method {
        HttpMethod::GET => RocketMethod::Get,
        HttpMethod::POST => RocketMethod::Post,
        HttpMethod::PUT => RocketMethod::Put,
        HttpMethod::DELETE => RocketMethod::Delete,
        HttpMethod::PATCH => RocketMethod::Patch,
        HttpMethod::HEAD => RocketMethod::Head,
        HttpMethod::OPTIONS => RocketMethod::Options,
        HttpMethod::TRACE => RocketMethod::Trace,
        HttpMethod::CONNECT => RocketMethod::Connect,
    }
}

/// Parse `(name, segment_index)` pairs out of a toni-style path. `:foo` and
/// `*foo` count as dynamic; everything else is literal. Rocket's
/// `Request::param(n)` keys off the **total** segment index (literals
/// included), not the rank among dynamic segments — get this wrong and the
/// extractor pulls the wrong slot. So we record the absolute index of each
/// dynamic name within the non-empty segments.
fn dynamic_param_names(toni_path: &str) -> Vec<(String, usize)> {
    let mut params = Vec::new();
    let mut idx = 0usize;
    for segment in toni_path.split('/') {
        let trimmed = segment.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(name) = trimmed.strip_prefix(':') {
            params.push((name.to_string(), idx));
        } else if let Some(name) = trimmed.strip_prefix('*') {
            params.push((name.to_string(), idx));
        }
        idx += 1;
    }
    params
}

/// Build an `http::request::Parts` from a rocket `Request`. Rocket exposes
/// method/uri/headers individually but no native conversion to `http`'s
/// `Parts`, so we reconstruct via the builder. Path-parameter names are
/// pre-parsed from the toni route at bind time (rocket's matched URI metadata
/// is `pub(crate)`, so we can't introspect it here) and indexed via
/// `req.param::<&str>(idx)`.
fn extract_parts(req: &RocketRequest<'_>, param_names: &[(String, usize)]) -> RequestPart {
    let method = http::Method::try_from(req.method().as_str()).unwrap_or(http::Method::GET);
    let uri_str = req.uri().to_string();
    let uri: http::Uri = uri_str.parse().unwrap_or_else(|_| http::Uri::default());

    let mut builder = http::Request::builder().method(method).uri(uri);

    if let Some(headers) = builder.headers_mut() {
        for header in req.headers().iter() {
            if let (Ok(name), Ok(value)) = (
                http::HeaderName::from_bytes(header.name().as_str().as_bytes()),
                http::HeaderValue::from_str(header.value()),
            ) {
                headers.append(name, value);
            }
        }
    }

    let req_built = builder.body(()).expect("valid request parts");
    let (mut http_parts, _) = req_built.into_parts();

    let mut params: HashMap<String, String> = HashMap::new();
    for (name, idx) in param_names {
        if let Some(Ok(value)) = req.param::<&str>(*idx) {
            params.insert(name.clone(), value.to_string());
        }
    }
    if !params.is_empty() {
        http_parts.extensions.insert(PathParams(params));
    }

    http_parts
}

/// Read rocket's `Data<'r>` into owned bytes. The `Data` borrows the request
/// stream so it can't outlive `'r` — we materialize eagerly inside the
/// handler future, which IS bounded by `'r`, and hand toni a buffered body.
async fn read_request_body(data: Data<'_>) -> Bytes {
    match data.open(REQUEST_BODY_LIMIT).into_bytes().await {
        Ok(capped) => Bytes::from(capped.into_inner()),
        Err(e) => {
            tracing::warn!(error = %e, "failed to read rocket request body");
            Bytes::new()
        }
    }
}

fn toni_response_to_rocket<'r>(http_res: HttpResponse) -> RocketResponse<'r> {
    let status = Status::from_code(http_res.status).unwrap_or(Status::InternalServerError);

    let mut builder = RocketResponse::build();
    builder.status(status);

    if let Some(body) = http_res.body.as_ref() {
        if let Some(ct) = body.content_type() {
            builder.raw_header("Content-Type", ct.to_owned());
        }
    }

    for (k, v) in &http_res.headers {
        builder.raw_header(k.clone(), v.clone());
    }

    if let Some(toni_body) = http_res.body {
        if toni_body.is_streaming() {
            // Bridge http_body::Body → Stream<Bytes> → AsyncRead and stream
            // chunks back without buffering. Rocket's `streamed_body` accepts
            // any `AsyncRead + 'r`; toni's `BoxBody` is `'static`, so the
            // lifetime constraint is trivially satisfied.
            let box_body = toni_body.into_box_body();
            let stream = futures_util::stream::unfold(box_body, |mut body| async move {
                use http_body_util::BodyExt;
                match body.frame().await {
                    Some(Ok(frame)) => match frame.into_data() {
                        Ok(data) => Some((Ok::<_, std::io::Error>(data), body)),
                        Err(_) => None,
                    },
                    Some(Err(e)) => Some((Err(std::io::Error::other(e)), body)),
                    None => None,
                }
            });
            let reader = tokio_util::io::StreamReader::new(stream);
            builder.streamed_body(reader);
        } else if let Some(bytes) = toni_body.try_bytes() {
            // Buffered body — known size, send via sized_body for correct
            // Content-Length without forcing the client into chunked.
            let len = bytes.len();
            builder.sized_body(len, Cursor::new(bytes.clone()));
        }
    }

    builder.finalize()
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

/// Rocket handler that bridges a single toni HTTP route through the adapter context.
#[derive(Clone)]
struct ToniRocketHandler {
    handler: Arc<dyn RequestHandler>,
    ctx: Arc<AdapterContext>,
    param_names: Arc<Vec<(String, usize)>>,
}

#[rocket::async_trait]
impl Handler for ToniRocketHandler {
    async fn handle<'r>(&self, req: &'r RocketRequest<'_>, data: Data<'r>) -> Outcome<'r> {
        let parts = extract_parts(req, &self.param_names);
        let body_bytes = read_request_body(data).await;
        let http_req = HttpRequest::from_parts(parts, RequestBody::Buffered(body_bytes));

        let handler = self.handler.clone();
        let http_res = self
            .ctx
            .execute(http_req, move |req| {
                let handler = handler.clone();
                Box::pin(async move { handler.handle(req).await })
            })
            .await;

        Outcome::Success(toni_response_to_rocket(http_res))
    }
}

/// Rocket handler for the catch-all 404 — runs the global chain so global
/// middleware can still observe the unmatched request.
#[derive(Clone)]
struct ToniRocketFallback {
    ctx: Arc<AdapterContext>,
}

#[rocket::async_trait]
impl Handler for ToniRocketFallback {
    async fn handle<'r>(&self, req: &'r RocketRequest<'_>, data: Data<'r>) -> Outcome<'r> {
        // The fallback never has named params — it only matches via the
        // `<__toni_fallback..>` wildcard, which we don't expose to handlers.
        let parts = extract_parts(req, &[]);
        let body_bytes = read_request_body(data).await;
        let http_req = HttpRequest::from_parts(parts, RequestBody::Buffered(body_bytes));

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

        Outcome::Success(toni_response_to_rocket(http_res))
    }
}

/// Rocket handler that performs a same-port WebSocket upgrade and pipes the
/// connection through toni's `WsConnectionCallbacks`.
#[derive(Clone)]
struct ToniRocketWsHandler {
    callbacks: Arc<WsConnectionCallbacks>,
    param_names: Arc<Vec<(String, usize)>>,
}

#[rocket::async_trait]
impl Handler for ToniRocketWsHandler {
    async fn handle<'r>(&self, req: &'r RocketRequest<'_>, _data: Data<'r>) -> Outcome<'r> {
        let parts = extract_parts(req, &self.param_names);
        let callbacks = self.callbacks.clone();

        let ws = match RocketWs::from_request(req).await {
            rocket::request::Outcome::Success(ws) => ws,
            rocket::request::Outcome::Forward(status) => {
                return Outcome::Error(status);
            }
            rocket::request::Outcome::Error((status, _)) => {
                return Outcome::Error(status);
            }
        };

        let channel = ws.channel(move |duplex| {
            Box::pin(async move {
                run_ws_connection(duplex, callbacks, parts).await;
                Ok(())
            })
        });

        Outcome::from(req, channel)
    }
}

async fn run_ws_connection(
    stream: rocket_ws::stream::DuplexStream,
    callbacks: Arc<WsConnectionCallbacks>,
    parts: RequestPart,
) {
    let (mut write, mut read) = stream.split();
    let (tx, mut rx) = tokio::sync::mpsc::channel::<WsMessage>(32);

    tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            match ws_message_to_rocket(msg) {
                Ok(rocket_msg) => {
                    if let Err(e) = write.send(rocket_msg).await {
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
            Ok(rocket_msg) => match rocket_to_ws_message(rocket_msg) {
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
                tracing::debug!(client_id = %client_id, error = %e, "rocket WebSocket read error");
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
impl HttpAdapter for RocketAdapter {
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

    async fn into_lifecycle(
        mut self: Box<Self>,
        port: u16,
        hostname: &str,
        ctx: AdapterContext,
    ) -> Result<HttpLifecycleHandle> {
        let routes = std::mem::take(&mut self.routes);
        let ws_routes = std::mem::take(&mut self.ws_routes);
        let ctx = Arc::new(ctx);
        let mut shutdown_rx = self.shutdown_tx.subscribe();
        let shutdown_tx = self.shutdown_tx.clone();

        let mut rocket_routes: Vec<Route> = Vec::new();
        for (method, path, handler) in routes {
            let rocket_path = to_rocket_path(&path);
            let param_names = Arc::new(dynamic_param_names(&path));
            let toni_handler = ToniRocketHandler {
                handler,
                ctx: ctx.clone(),
                param_names,
            };
            rocket_routes.push(Route::new(
                to_rocket_method(method),
                &rocket_path,
                toni_handler,
            ));
        }
        for (path, callbacks) in ws_routes {
            let rocket_path = to_rocket_path(&path);
            let param_names = Arc::new(dynamic_param_names(&path));
            let ws_handler = ToniRocketWsHandler {
                callbacks,
                param_names,
            };
            // Rocket dispatches WebSocket upgrades through GET — same wire
            // contract as RFC 6455 expects.
            rocket_routes.push(Route::new(RocketMethod::Get, &rocket_path, ws_handler));
        }

        // Catch-all fallback — rank low so specific routes win. Rocket's
        // `<path..>` segment matches any tail.
        let fallback = ToniRocketFallback { ctx: ctx.clone() };
        let mut fallback_route =
            Route::new(RocketMethod::Get, "/<__toni_fallback..>", fallback.clone());
        fallback_route.rank = isize::MAX;
        rocket_routes.push(fallback_route);

        let address: std::net::IpAddr = hostname
            .parse()
            .unwrap_or(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
        let figment = Config::figment()
            .merge(("address", address))
            .merge(("port", port))
            .merge(("log_level", rocket::config::LogLevel::Off))
            .merge(("shutdown.ctrlc", false));

        // Rocket's `bind`-then-`local_addr` flow is `pub(crate)`. The only
        // public hook that fires after binding but before serving is a
        // liftoff fairing — use a oneshot to ferry the OS-assigned address
        // back to the toni runtime.
        let (addr_tx, addr_rx) = tokio::sync::oneshot::channel::<std::net::SocketAddr>();
        let addr_tx = std::sync::Arc::new(std::sync::Mutex::new(Some(addr_tx)));
        let liftoff = AdHoc::on_liftoff("toni-rocket bound", move |rocket| {
            let addr_tx = addr_tx.clone();
            Box::pin(async move {
                let cfg = rocket.config();
                let addr = std::net::SocketAddr::new(cfg.address, cfg.port);
                if let Some(tx) = addr_tx.lock().unwrap().take() {
                    let _ = tx.send(addr);
                }
            })
        });

        // `shutdown()` is only available on `Rocket<Ignite>` (and Orbit),
        // not `Build`, so ignite explicitly to grab the handle before
        // launching.
        let rocket = rocket::custom(figment)
            .mount("/", rocket_routes)
            .attach(liftoff)
            .ignite()
            .await
            .map_err(|e| anyhow!("rocket failed to ignite: {}", e))?;
        let shutdown_handle = rocket.shutdown();

        // Forward toni's shutdown signal to rocket's notify().
        tokio::spawn(async move {
            let _ = shutdown_rx.wait_for(|v| *v).await;
            shutdown_handle.notify();
        });

        let serve_task = tokio::spawn(async move {
            if let Err(e) = rocket.launch().await {
                tracing::error!(error = %e, "rocket server error");
            }
        });

        let local_addr = addr_rx
            .await
            .map_err(|_| anyhow!("rocket liftoff fairing did not fire — bind failed"))?;

        let serve = Box::pin(async move {
            let _ = serve_task.await;
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
