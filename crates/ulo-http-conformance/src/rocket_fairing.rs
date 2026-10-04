//! The test fairing that copies `Routing` from rocket's request-local cache onto a response header
//! the suite reads, since rocket's `Response` has no extensions and every declared capability is
//! asserted.

use rocket::fairing::{Fairing, Info, Kind};
use rocket::{Request, Response};

/// Attach to the host's `Rocket<Build>` in the adapter's conformance host.
#[derive(Clone, Copy, Debug, Default)]
pub struct RoutingFairing;

#[rocket::async_trait]
impl Fairing for RoutingFairing {
    fn info(&self) -> Info {
        Info { name: "ulo routing", kind: Kind::Response }
    }

    async fn on_response<'r>(&self, req: &'r Request<'_>, res: &mut Response<'r>) {
        let _ = (req, res);
        todo!()
    }
}
