//! Scenarios: routing.

use crate::{Host, Mode};

/// A request a route matches, answered by its handler.
pub async fn hit<H: Host>(mode: Mode) {
    let _ = mode;
    todo!()
}

/// A path nothing matches: 404 with `NoRoute`, `Routing::NotFound`.
pub async fn not_found<H: Host>(mode: Mode) {
    let _ = mode;
    todo!()
}

/// A matching path with the wrong method: 405 with `Allow`.
pub async fn method_not_allowed<H: Host>(mode: Mode) {
    let _ = mode;
    todo!()
}

/// `OPTIONS` without a handler: 204 with `Allow`, `Routing::Options`.
pub async fn options<H: Host>(mode: Mode) {
    let _ = mode;
    todo!()
}

/// `HEAD` answered from the `GET` handler, body omitted.
pub async fn head<H: Host>(mode: Mode) {
    let _ = mode;
    todo!()
}
