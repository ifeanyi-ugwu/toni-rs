use std::pin::Pin;

use crate::http_helpers::{HttpRequest, HttpResponse};

pub type BoxFuture<T> = Pin<Box<dyn std::future::Future<Output = T> + Send>>;

/// The per-route handler the framework registers with an adapter via `bind`.
///
/// Each `Arc<dyn RequestHandler>` wraps one route's pipeline: route-scoped
/// middleware → guards → interceptors → pipes → controller.  The adapter
/// stores these at bootstrap and dispatches to the matching one at request
/// time.
pub trait RequestHandler: Send + Sync + 'static {
    fn handle(&self, req: HttpRequest) -> BoxFuture<HttpResponse>;
}
