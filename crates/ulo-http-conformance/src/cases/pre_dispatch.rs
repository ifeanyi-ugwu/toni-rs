//! Scenarios: pre dispatch.

use http::Method;

use crate::wire::{Exchange, is_reference, same_as_reference, start};
use crate::{Host, Mode, ORIGIN};

/// An unscoped entry answering a CORS preflight without calling `next`, `Routing::Unrouted`.
pub async fn preflight<H: Host>(mode: Mode) {
    let host = start::<H>(mode).await;
    let exchange = Exchange::new(Method::OPTIONS, "/hit")
        .header("origin", ORIGIN)
        .header("access-control-request-method", "GET");
    let reply = same_as_reference(&host, exchange).await;
    assert_eq!(reply.status, 204);
    assert_eq!(reply.header("access-control-allow-origin"), Some(ORIGIN));
    if !is_reference::<H>() {
        assert_eq!(reply.routing(), Some("unrouted"));
    }
    host.stop().await;
}
