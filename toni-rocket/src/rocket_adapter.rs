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
    AdapterContext, BindTarget, Body as ToniBody, HttpAdapter, HttpLifecycleHandle, HttpMethod,
    HttpRequest, HttpResponse, MessageCallbackResult, RequestHandler, WsConnectionCallbacks,
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

/// Match a toni-form path pattern (`/users/{id}` or `/users/:id`,
/// `/files/*tail`) against a concrete path, returning the captured
/// parameters on match. Routing is internal to this adapter — rocket's
/// router cannot host the pre-routing chain (fairings cannot short-circuit),
/// so one catch-all route per method dispatches through this instead.
fn match_route(pattern: &str, path: &str) -> Option<HashMap<String, String>> {
    let pattern_segs: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
    let path_segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

    let mut params = HashMap::new();
    for (i, p) in pattern_segs.iter().enumerate() {
        if let Some(name) = p.strip_prefix('*') {
            params.insert(name.to_string(), path_segs[i..].join("/"));
            return Some(params);
        }
        let param_name = p
            .strip_prefix(':')
            .or_else(|| p.strip_prefix('{').and_then(|s| s.strip_suffix('}')));
        match path_segs.get(i) {
            Some(s) => {
                if let Some(name) = param_name {
                    params.insert(name.to_string(), s.to_string());
                } else if p != s {
                    return None;
                }
            }
            None => return None,
        }
    }
    (path_segs.len() == pattern_segs.len()).then_some(params)
}

/// Build an `http::request::Parts` from a rocket `Request`. Rocket exposes
/// method/uri/headers individually but no native conversion to `http`'s
/// `Parts`, so we reconstruct via the builder. Path parameters are not read
/// here — internal routing captures them via [`match_route`].
fn extract_parts(req: &RocketRequest<'_>) -> RequestPart {
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
    let (http_parts, _) = req_built.into_parts();
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
            "error": match status {
                404 => "Not Found",
                405 => "Method Not Allowed",
                _ => "Internal Server Error",
            },
        }))),
    }
}

/// Internal marker on a routing-closure response signalling "this path is a
/// WebSocket route — perform the upgrade". The closure cannot upgrade itself:
/// `RocketWs::from_request` needs the borrowed rocket request, which a
/// `'static` closure cannot hold. The marker carries the matched route index
/// and captured params; the outer handler upgrades only if the marker
/// survived the chain, so middleware can reject upgrades by replacing the
/// response.
const WS_MARKER_HEADER: &str = "x-toni-rocket-ws-route";
const WS_PARAMS_HEADER: &str = "x-toni-rocket-ws-params";

fn ws_marker_response(index: usize, params: &HashMap<String, String>) -> HttpResponse {
    let mut res = json_error_response(500, "WebSocket upgrade not performed".into());
    res.headers
        .push((WS_MARKER_HEADER.into(), index.to_string()));
    if !params.is_empty() {
        if let Ok(encoded) = serde_json::to_string(params) {
            res.headers.push((WS_PARAMS_HEADER.into(), encoded));
        }
    }
    res
}

fn take_ws_marker(res: &HttpResponse) -> Option<(usize, HashMap<String, String>)> {
    let index = res
        .headers
        .iter()
        .find(|(k, _)| k == WS_MARKER_HEADER)?
        .1
        .parse()
        .ok()?;
    let params = res
        .headers
        .iter()
        .find(|(k, _)| k == WS_PARAMS_HEADER)
        .and_then(|(_, v)| serde_json::from_str(v).ok())
        .unwrap_or_default();
    Some((index, params))
}

/// The single rocket handler, mounted as a catch-all for every method: runs
/// the global middleware chain once per request, before any route matching.
/// The request the chain forwards is the one routing matches on, so
/// middleware can rewrite paths, short-circuit (auth, CORS preflight), and
/// observe every response — including 404s and 405s. Routing itself is
/// internal ([`match_route`]); rocket serves connections and performs
/// WebSocket upgrades.
#[derive(Clone)]
struct GlobalChainHandler {
    ctx: Arc<AdapterContext>,
    http_routes: Arc<Vec<(HttpMethod, String, Arc<dyn RequestHandler>)>>,
    ws_routes: Arc<Vec<(String, Arc<WsConnectionCallbacks>)>>,
}

async fn dispatch(
    treq: HttpRequest,
    http_routes: &[(HttpMethod, String, Arc<dyn RequestHandler>)],
    ws_routes: &[(String, Arc<WsConnectionCallbacks>)],
) -> HttpResponse {
    let method = treq.method().as_str().to_uppercase();
    let path = treq.uri().path().to_string();

    // WebSocket upgrades dispatch through GET — RFC 6455 wire contract.
    if method == "GET" {
        for (i, (pattern, _)) in ws_routes.iter().enumerate() {
            if let Some(params) = match_route(pattern, &path) {
                return ws_marker_response(i, &params);
            }
        }
    }

    let mut matched = None;
    for (route_method, pattern, handler) in http_routes.iter() {
        if format!("{:?}", route_method) != method {
            continue;
        }
        if let Some(params) = match_route(pattern, &path) {
            matched = Some((handler, params));
            break;
        }
    }
    if let Some((handler, params)) = matched {
        let mut treq = treq;
        if !params.is_empty() {
            treq.extensions_mut().insert(PathParams(params));
        }
        return handler.handle(treq).await;
    }

    let allowed: Vec<String> = http_routes
        .iter()
        .filter(|(_, pattern, _)| match_route(pattern, &path).is_some())
        .map(|(m, _, _)| format!("{:?}", m))
        .collect();

    if allowed.is_empty() {
        json_error_response(404, format!("Cannot {} {}", method, path))
    } else {
        let mut res =
            json_error_response(405, format!("Method {} not allowed for {}", method, path));
        res.headers.push(("allow".into(), allowed.join(", ")));
        res
    }
}

#[rocket::async_trait]
impl Handler for GlobalChainHandler {
    async fn handle<'r>(&self, req: &'r RocketRequest<'_>, data: Data<'r>) -> Outcome<'r> {
        let parts = extract_parts(req);
        let body_bytes = read_request_body(data).await;
        let http_req = HttpRequest::from_parts(parts, RequestBody::Buffered(body_bytes));

        let http_routes = self.http_routes.clone();
        let ws_routes = self.ws_routes.clone();
        let http_res = self
            .ctx
            .execute(http_req, move |treq| {
                Box::pin(async move { dispatch(treq, &http_routes, &ws_routes).await })
            })
            .await;

        if let Some((index, params)) = take_ws_marker(&http_res) {
            let Some((_, callbacks)) = self.ws_routes.get(index) else {
                return Outcome::Error(Status::InternalServerError);
            };
            let callbacks = callbacks.clone();

            let mut parts = extract_parts(req);
            if !params.is_empty() {
                parts.extensions.insert(PathParams(params));
            }

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

            return Outcome::from(req, channel);
        }

        Outcome::Success(toni_response_to_rocket(http_res))
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
        // Rocket fuses bind and serve into `launch()` with no public hook for
        // an existing listener, so only address targets are supported.
        let (hostname, port) = match target {
            BindTarget::Addr { hostname, port } => (hostname, port),
            other => anyhow::bail!(
                "RocketAdapter cannot adopt a {}; rocket binds internally from \
                 figment config — pass a (host, port) address instead",
                other
            ),
        };
        let routes = std::mem::take(&mut self.routes);
        let ws_routes = std::mem::take(&mut self.ws_routes);
        let mut shutdown_rx = self.shutdown_tx.subscribe();
        let shutdown_tx = self.shutdown_tx.clone();

        // One catch-all per method; routing is internal to the handler. The
        // chain must observe every request, and rocket offers no pre-routing
        // anchor (fairings cannot short-circuit), so rocket's router reduces
        // to connection serving.
        let chain = GlobalChainHandler {
            ctx: Arc::new(ctx),
            http_routes: Arc::new(routes),
            ws_routes: Arc::new(ws_routes),
        };
        let rocket_routes: Vec<Route> = [
            RocketMethod::Get,
            RocketMethod::Post,
            RocketMethod::Put,
            RocketMethod::Delete,
            RocketMethod::Patch,
            RocketMethod::Head,
            RocketMethod::Options,
            RocketMethod::Trace,
            RocketMethod::Connect,
        ]
        .into_iter()
        .map(|m| Route::new(m, "/<__toni_chain..>", chain.clone()))
        .collect();

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
