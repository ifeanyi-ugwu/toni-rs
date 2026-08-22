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
    let m = ctx.metadata();
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

// ---- accumulating rather than replacing --------------------------------------

#[controller("/accumulate")]
pub struct AccumulateController {}

/// The impl block's `Roles` is not lost when a handler declares its own — it is the earlier of two
/// entries, and a reader that wants both asks for both.
#[routes]
#[set_metadata(Roles(vec!["authenticated"]))]
impl AccumulateController {
    /// Declares nothing, so one entry exists.
    #[get("/inherited")]
    fn inherited(&self, ctx: &HttpContext) -> ToniBody {
        ToniBody::text(read_roles(ctx))
    }

    /// Declares its own, so two exist: the block's first, this one second.
    #[get("/added")]
    #[set_metadata(Roles(vec!["admin"]))]
    fn added(&self, ctx: &HttpContext) -> ToniBody {
        ToniBody::text(read_roles(ctx))
    }
}

/// `get` answers the winner and `get_all` answers every declaration, least-specific first.
fn read_roles(ctx: &HttpContext) -> String {
    let m = ctx.metadata().expect("declared metadata");
    let winner = m.get::<Roles>().map(|r| r.0.join("+")).unwrap_or_default();
    let all: Vec<String> = m.get_all::<Roles>().iter().map(|r| r.0.join("+")).collect();
    format!("{winner}|{}", all.join(","))
}

#[derive(Clone)]
pub struct Roles(pub Vec<&'static str>);

#[module(controllers: [AccumulateController])]
impl AccumulateModule {}

#[tokio_localset_test::localset_test]
async fn a_handler_declaration_does_not_erase_the_blocks() {
    let server = TestServer::start(AccumulateModule).await;
    assert_eq!(
        fetch(&server, "/accumulate/inherited").await,
        "authenticated|authenticated"
    );
    assert_eq!(
        fetch(&server, "/accumulate/added").await,
        "admin|authenticated,admin",
        "`get` gives the handler's and `get_all` gives both, block first"
    );
}
