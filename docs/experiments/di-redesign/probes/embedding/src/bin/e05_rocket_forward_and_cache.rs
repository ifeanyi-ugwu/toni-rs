//! A rocket `Handler` can forward a miss only with the original `Data<'r>`: `Data::local` is
//! `pub(crate)`, so a body already read cannot be rebuilt for the forward. It can leave a typed value
//! for the host in the request's local cache; rocket's `Request` carries no response extensions.
//! Compile-level, plus rocket's shutdown defaults printed.

use rocket::data::Data;
use rocket::http::Status;
use rocket::request::Request;
use rocket::route::{Handler, Outcome};

#[derive(Clone, Copy, Debug)]
struct Handled {
    route: &'static str,
}

#[derive(Clone)]
struct Embed;

#[rocket::async_trait]
impl Handler for Embed {
    async fn handle<'r>(&self, req: &'r Request<'_>, data: Data<'r>) -> Outcome<'r> {
        let handled: &Handled = req.local_cache(|| Handled { route: "/users/{id}" });
        let _ = handled.route;
        // The app routed nothing: hand the request back with its body unread.
        Outcome::Forward((data, Status::NotFound))
    }
}

fn main() {
    let shutdown_default = rocket::config::Shutdown::default();
    println!("ctrlc={} grace={} mercy={}", shutdown_default.ctrlc, shutdown_default.grace, shutdown_default.mercy);
    let _route = rocket::Route::ranked(10, rocket::http::Method::Get, "/<path..>", Embed);
    println!("rocket forward and local_cache compile");
}
