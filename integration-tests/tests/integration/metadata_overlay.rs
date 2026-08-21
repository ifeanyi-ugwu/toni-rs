//! Declared metadata comes from the impl block and the handler, with the handler winning.
//!
//! The class half existed nowhere before: `#[set_metadata]` was read from method attributes only, so
//! an annotation on the impl block compiled and did nothing.

use toni::context::{HandlerContext, HttpContext};
use toni::{controller, get, module, routes, set_metadata, Body as ToniBody};

use crate::common::TestServer;

#[derive(Clone, Debug, PartialEq)]
pub struct Tier(&'static str);

#[derive(Clone, Debug, PartialEq)]
pub struct Audience(&'static str);

#[controller("/meta")]
pub struct MetaController {}

/// Both entries apply to every handler below unless one overrides them.
#[routes]
#[set_metadata(Tier("standard"))]
#[set_metadata(Audience("internal"))]
impl MetaController {
    /// Inherits both.
    #[get("/inherited")]
    fn inherited(&self, ctx: &HttpContext) -> ToniBody {
        ToniBody::text(read(ctx))
    }

    /// Overrides one and inherits the other.
    #[get("/overridden")]
    #[set_metadata(Tier("premium"))]
    fn overridden(&self, ctx: &HttpContext) -> ToniBody {
        ToniBody::text(read(ctx))
    }
}

fn read(ctx: &HttpContext) -> String {
    let m = ctx.route_metadata();
    let tier = m
        .and_then(|m| m.get::<Tier>())
        .map(|t| t.0)
        .unwrap_or("none");
    let audience = m
        .and_then(|m| m.get::<Audience>())
        .map(|a| a.0)
        .unwrap_or("none");
    format!("{tier}/{audience}")
}

#[module(controllers: [MetaController])]
impl MetaModule {}

async fn fetch(server: &TestServer, path: &str) -> String {
    server
        .client()
        .get(server.url(path))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap()
}

#[tokio_localset_test::localset_test]
async fn a_handler_inherits_what_the_impl_block_declares() {
    let server = TestServer::start(MetaModule).await;
    assert_eq!(fetch(&server, "/meta/inherited").await, "standard/internal");
}

#[tokio_localset_test::localset_test]
async fn a_handler_overrides_one_entry_and_keeps_the_rest() {
    let server = TestServer::start(MetaModule).await;
    assert_eq!(fetch(&server, "/meta/overridden").await, "premium/internal");
}
