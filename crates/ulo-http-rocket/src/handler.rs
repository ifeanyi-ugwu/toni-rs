//! The catch-all routes: one `rocket::route::Handler` per method, ranked so rocket's own routes
//! answer first, building the app's `Request` with the prefix stripped and the body buffered, and
//! forwarding a `Forwardable` miss with the request's original `Data`.

use rocket::route::{Handler, Outcome, Route};
use rocket::{Data, Request};

use crate::Handle;

/// The handler every catch-all route carries.
#[derive(Clone)]
pub struct RocketHandler {
    pub(crate) handle: Handle,
}

/// One catch-all route per method over `handle`, for `rocket.mount("/api", routes(&embedded))`.
pub fn routes(handle: &Handle) -> Vec<Route> {
    let _ = handle;
    todo!()
}

#[rocket::async_trait]
impl Handler for RocketHandler {
    async fn handle<'r>(&self, request: &'r Request<'_>, data: Data<'r>) -> Outcome<'r> {
        let _ = (request, data, &self.handle);
        todo!()
    }
}
