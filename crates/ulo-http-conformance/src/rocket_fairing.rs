//! The test fairing that copies `Routing` from rocket's request-local cache onto a response header
//! the suite reads, since rocket's `Response` has no extensions and every declared capability is
//! asserted.

use rocket::fairing::{Fairing, Info, Kind};
use rocket::{Request, Response};
use ulo_http::Routing;

use crate::{ROUTING_HEADER, routing_label};

/// Attach to the host's `Rocket<Build>` in the adapter's conformance host. It reads the
/// `Option<Routing>` the rocket adapter caches per request and writes [`ROUTING_HEADER`]; a
/// response the app did not give finds `None` and gets no header.
#[derive(Clone, Copy, Debug, Default)]
pub struct RoutingFairing;

#[rocket::async_trait]
impl Fairing for RoutingFairing {
    fn info(&self) -> Info {
        Info { name: "ulo routing", kind: Kind::Response }
    }

    async fn on_response<'r>(&self, req: &'r Request<'_>, res: &mut Response<'r>) {
        if let Some(routing) = req.local_cache(|| None::<Routing>) {
            res.set_raw_header(ROUTING_HEADER, routing_label(routing));
        }
    }
}
