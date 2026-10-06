//! The `HttpServiceFactory` over the app: a scope at a path, or the default service, building the
//! app's `Request` from actix's with the payload pumped, and answering through
//! `Service::respond`, which strips the prefix actix leaves on the path.

use std::borrow::Cow;
use std::future::{Ready, ready};
use std::task::{Context, Poll};

use actix_http::ConnectionType;
use actix_service::{Service, ServiceFactory};
use actix_web::body::BoxBody;
use actix_web::dev::{AppService, HttpServiceFactory, ServiceRequest, ServiceResponse};
use actix_web::http::header::{HeaderName as ActixName, HeaderValue as ActixValue};
use actix_web::{HttpRequest, HttpResponse};
use futures_util::future::LocalBoxFuture;
use http::header::CONNECTION;
use ulo_http::{ConnInfo, Request, Routing};

use crate::Handle;
use crate::pump::{ResponseBody, request_body};

/// The service an actix `App` mounts, from [`scope`].
#[derive(Clone)]
pub struct ActixScope {
    pub(crate) path: Cow<'static, str>,
    pub(crate) handle: Handle,
}

/// The app at `path`, `App::service(ulo_http_actix::scope("/api", &embedded))`, every method and
/// every path under it. As the fallback, `App::default_service(ulo_http_actix::scope("", &embedded))`,
/// which answers what no other service matched; `App::service` with `""` mounts it ahead of the
/// services registered after it instead.
pub fn scope(path: impl Into<Cow<'static, str>>, handle: &Handle) -> ActixScope {
    ActixScope { path: path.into(), handle: handle.clone() }
}

impl HttpServiceFactory for ActixScope {
    fn register(self, config: &mut AppService) {
        let path = self.path.clone();
        HttpServiceFactory::register(actix_web::web::scope(&path).default_service(self), config);
    }
}

impl ServiceFactory<ServiceRequest> for ActixScope {
    type Response = ServiceResponse;
    type Error = actix_web::Error;
    type Config = ();
    type Service = ActixService;
    type InitError = ();
    type Future = Ready<Result<ActixService, ()>>;

    fn new_service(&self, _: ()) -> Self::Future {
        ready(Ok(ActixService { handle: self.handle.clone() }))
    }
}

/// The per-worker service an [`ActixScope`] builds.
pub struct ActixService {
    handle: Handle,
}

impl Service<ServiceRequest> for ActixService {
    type Response = ServiceResponse;
    type Error = actix_web::Error;
    type Future = LocalBoxFuture<'static, Result<ServiceResponse, actix_web::Error>>;

    fn poll_ready(&self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&self, req: ServiceRequest) -> Self::Future {
        let (host, payload) = req.into_parts();
        let Some(head) = head(&host) else {
            return Box::pin(async move { Ok(ServiceResponse::new(host, HttpResponse::BadRequest().finish())) });
        };
        let conn = ConnInfo::new(head.version).local(host.app_config().local_addr());
        let conn = match host.peer_addr() {
            Some(peer) => conn.peer(peer),
            None => conn,
        };
        let app_req = Request { head, body: request_body(payload), conn, upgrade: None };
        let answer = self.handle.service().respond(&host, app_req);
        Box::pin(async move {
            let response = answer.await;
            Ok(ServiceResponse::new(host, response_from(response)))
        })
    }
}

/// actix's request head, on `http` 0.2, as the app's on `http` 1. `None` for a URI that does not
/// re-parse, which actix parsed already and so never yields.
fn head(host: &HttpRequest) -> Option<http::request::Parts> {
    let (mut head, ()) = http::Request::new(()).into_parts();
    head.method = http::Method::from_bytes(host.method().as_str().as_bytes()).ok()?;
    head.uri = http::Uri::try_from(host.uri().to_string()).ok()?;
    head.version = version(host.version());
    for (name, value) in host.headers() {
        let name = http::HeaderName::from_bytes(name.as_str().as_bytes()).ok()?;
        let value = http::HeaderValue::from_bytes(value.as_bytes()).ok()?;
        head.headers.append(name, value);
    }
    Some(head)
}

fn version(version: actix_web::http::Version) -> http::Version {
    use actix_web::http::Version as Actix;
    if version == Actix::HTTP_09 {
        http::Version::HTTP_09
    } else if version == Actix::HTTP_10 {
        http::Version::HTTP_10
    } else if version == Actix::HTTP_2 {
        http::Version::HTTP_2
    } else if version == Actix::HTTP_3 {
        http::Version::HTTP_3
    } else {
        http::Version::HTTP_11
    }
}

/// The app's response as actix's. actix's response extensions are its own store, so `Routing` is
/// copied into them and the app's other extensions stay behind. A `Connection: close` header sets
/// the head's connection type, since actix-http's HTTP/1 encoder writes the connection line from
/// that flag and skips the header.
fn response_from(response: ulo_http::Response) -> HttpResponse<BoxBody> {
    let (parts, body) = response.into_parts();
    let status = actix_web::http::StatusCode::from_u16(parts.status.as_u16())
        .unwrap_or(actix_web::http::StatusCode::INTERNAL_SERVER_ERROR);
    let mut res = HttpResponse::with_body(status, ResponseBody(body));
    for (name, value) in &parts.headers {
        if let (Ok(name), Ok(value)) =
            (ActixName::from_bytes(name.as_str().as_bytes()), ActixValue::from_bytes(value.as_bytes()))
        {
            res.headers_mut().append(name, value);
        }
    }
    if closes(&parts.headers) {
        res.head_mut().set_connection_type(ConnectionType::Close);
    }
    if let Some(routing) = parts.extensions.get::<Routing>() {
        res.extensions_mut().insert(routing.clone());
    }
    res.map_into_boxed_body()
}

fn closes(headers: &http::HeaderMap) -> bool {
    headers.get_all(CONNECTION).iter().any(|value| {
        value.to_str().is_ok_and(|value| value.split(',').any(|token| token.trim().eq_ignore_ascii_case("close")))
    })
}
