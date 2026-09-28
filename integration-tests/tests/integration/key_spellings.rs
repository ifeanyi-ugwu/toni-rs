//! The key spellings `#[inject(..)]` and the `#[use_*]` attributes accept, each resolved end to end.
//!
//! Every key position reads a type: a marker bare or qualified, a type written as its own key,
//! capitals included, `dyn Trait`, or a generic type with or without a turbofish. The `#[use_*]` attributes also take
//! a value held in a const, written `value X`, and one held in a `static`, written `value &X`. The
//! refused spellings are the `ulo-macros` diagnostics cases.

use std::marker::PhantomData;
use std::sync::Arc;

use ulo::di::Key;
use ulo::enhancer::{ChainError, ErrorHandler, Guard, Interceptor, InterceptorNext};
use ulo::http::{Body, HttpContext, HttpError, HttpHandlerResult, HttpResponse};
use ulo::{
    async_trait, controller, get, injectable, key, module, new, provide, routes,
    use_error_handlers, use_guards, use_interceptors,
};

use crate::common::TestServer;

pub trait Plugin: Send + Sync {
    fn name(&self) -> String;
}

pub struct Alpha;

impl Plugin for Alpha {
    fn name(&self) -> String {
        "alpha".to_string()
    }
}

pub struct Beta;

impl Plugin for Beta {
    fn name(&self) -> String {
        "beta".to_string()
    }
}

mod keys {
    use super::*;

    key!(pub Port: u16);
    key!(pub Auth: dyn Guard<HttpContext>);
    key!(pub Plugins: dyn Plugin);
}

key!(pub Name: String);
key!(pub LocalAuth: dyn Guard<HttpContext>);

/// Generic types written as keys, a slot per set of arguments.
pub struct Tagged<T>(PhantomData<T>);
pub struct Pair<A, B>(PhantomData<(A, B)>);
pub struct Admin;
pub struct Staff;

impl Key for Tagged<Admin> {
    type Value = dyn Guard<HttpContext>;
}

impl Key for Pair<Admin, Staff> {
    type Value = dyn Guard<HttpContext>;
}

impl Key for Tagged<Staff> {
    type Value = String;
}

/// Admits a request carrying its header.
#[derive(Clone)]
pub struct HeaderGuard(&'static str);

#[async_trait]
impl Guard<HttpContext> for HeaderGuard {
    async fn can_activate(&self, ctx: &HttpContext) -> bool {
        ctx.request().headers.contains_key(self.0)
    }
}

const ADMIN: HeaderGuard = HeaderGuard("x-admin");
static LIMITER: HeaderGuard = HeaderGuard("x-static");

/// Stamps every answer it passes through.
pub struct Stamp;

#[async_trait]
impl Interceptor<HttpContext, HttpHandlerResult> for Stamp {
    async fn intercept(
        &self,
        ctx: &HttpContext,
        next: Box<dyn InterceptorNext<HttpContext, HttpHandlerResult>>,
    ) -> HttpHandlerResult {
        let mut answer = next.run(ctx).await?;
        answer
            .headers
            .push(("x-stamp".to_string(), "1".to_string()));
        Ok(answer)
    }
}

const TIMING: Stamp = Stamp;
static STAMP: Stamp = Stamp;

/// Answers every failure with a 418.
pub struct Teapot;

#[async_trait]
impl ErrorHandler<HttpContext, HttpHandlerResult> for Teapot {
    async fn handle_error(
        &self,
        _error: ChainError<'_>,
        _ctx: &HttpContext,
    ) -> Option<HttpHandlerResult> {
        let mut answer = HttpResponse::new();
        answer.status = 418;
        Some(Ok(answer))
    }
}

const FALLBACK: Teapot = Teapot;
static TEAPOT: Teapot = Teapot;

#[injectable]
pub struct Plain {}

#[async_trait]
impl Guard<HttpContext> for Plain {
    async fn can_activate(&self, ctx: &HttpContext) -> bool {
        ctx.request().headers.contains_key("x-plain")
    }
}

/// A type written in capitals is a type like any other bare path.
#[injectable]
#[allow(clippy::upper_case_acronyms)]
pub struct HTTP {}

#[async_trait]
impl Guard<HttpContext> for HTTP {
    async fn can_activate(&self, ctx: &HttpContext) -> bool {
        ctx.request().headers.contains_key("x-http")
    }
}

mod guards {
    use super::*;

    #[injectable]
    pub struct Qualified {}

    #[async_trait]
    impl Guard<HttpContext> for Qualified {
        async fn can_activate(&self, ctx: &HttpContext) -> bool {
            ctx.request().headers.contains_key("x-qualified")
        }
    }
}

#[injectable]
pub struct Fields {
    #[inject(Name)]
    by_bare_marker: String,
    #[inject(keys::Port)]
    by_qualified_marker: u16,
    #[inject(keys::Plugins)]
    by_collection_marker: Vec<Arc<dyn Plugin>>,
    #[inject(dyn Plugin)]
    by_trait_object: Vec<Arc<dyn Plugin>>,
    #[inject(Plain)]
    by_own_type: Plain,
    #[inject(HTTP)]
    by_capitals: HTTP,
    #[inject(Tagged<Staff>)]
    by_generic: String,
}

#[injectable]
pub struct Parameters {
    port: u16,
}

impl Parameters {
    #[new]
    fn new(#[inject(keys::Port)] port: u16) -> Self {
        Self { port }
    }
}

#[controller("/spell")]
pub struct SpellingController {
    #[inject]
    fields: Fields,
    #[inject]
    parameters: Parameters,
}

#[routes]
impl SpellingController {
    #[get("/keys")]
    fn keys(&self) -> Body {
        let f = &self.fields;
        let _ = (&f.by_own_type, &f.by_capitals);
        Body::text(format!(
            "{} {} {} {} {} {}",
            f.by_bare_marker,
            f.by_qualified_marker,
            f.by_collection_marker[0].name(),
            f.by_trait_object[0].name(),
            f.by_generic,
            self.parameters.port,
        ))
    }

    #[get("/type")]
    #[use_guards(Plain)]
    fn by_type(&self) -> Body {
        Body::text("ok".to_string())
    }

    #[get("/qualified-type")]
    #[use_guards(guards::Qualified)]
    fn by_qualified_type(&self) -> Body {
        Body::text("ok".to_string())
    }

    #[get("/capitals-type")]
    #[use_guards(HTTP)]
    fn by_capitals_type(&self) -> Body {
        Body::text("ok".to_string())
    }

    #[get("/qualified-marker")]
    #[use_guards(keys::Auth)]
    fn by_qualified_marker(&self) -> Body {
        Body::text("ok".to_string())
    }

    #[get("/bare-marker")]
    #[use_guards(LocalAuth)]
    fn by_bare_marker(&self) -> Body {
        Body::text("ok".to_string())
    }

    #[get("/generic")]
    #[use_guards(Tagged<Admin>)]
    fn by_generic(&self) -> Body {
        Body::text("ok".to_string())
    }

    #[get("/turbofish")]
    #[use_guards(Tagged::<Admin>)]
    fn by_turbofish(&self) -> Body {
        Body::text("ok".to_string())
    }

    #[get("/two-arguments")]
    #[use_guards(Pair<Admin, Staff>, value ADMIN)]
    fn by_two_arguments(&self) -> Body {
        Body::text("ok".to_string())
    }

    #[get("/const-value")]
    #[use_guards(value ADMIN)]
    fn by_const_value(&self) -> Body {
        Body::text("ok".to_string())
    }

    #[get("/static-value")]
    #[use_guards(value &LIMITER)]
    fn by_static_value(&self) -> Body {
        Body::text("ok".to_string())
    }

    #[get("/static-interceptor")]
    #[use_interceptors(value &STAMP)]
    fn by_static_interceptor(&self) -> Body {
        Body::text("ok".to_string())
    }

    #[get("/static-error-handler")]
    #[use_error_handlers(value &TEAPOT)]
    fn by_static_error_handler(&self) -> Result<Body, HttpError> {
        Err(HttpError::bad_request("refused"))
    }

    #[get("/const-interceptor")]
    #[use_interceptors(value TIMING)]
    fn by_const_interceptor(&self) -> Body {
        Body::text("ok".to_string())
    }

    #[get("/const-error-handler")]
    #[use_error_handlers(value FALLBACK)]
    fn by_const_error_handler(&self) -> Result<Body, HttpError> {
        Err(HttpError::bad_request("refused"))
    }
}

#[module(
    controllers: [SpellingController],
    providers: [
        Plain,
        HTTP,
        guards::Qualified,
        provide!(keys::Port => 8080u16),
        provide!(Name => "ulo".to_string()),
        provide!(into keys::Plugins => value Alpha),
        provide!(into dyn Plugin => value Beta),
        provide!(keys::Auth => HeaderGuard("x-auth")),
        provide!(LocalAuth => HeaderGuard("x-local")),
        provide!(Tagged<Admin> => HeaderGuard("x-tagged")),
        provide!(Pair<Admin, Staff> => HeaderGuard("x-pair")),
        provide!(Tagged<Staff> => "tagged".to_string()),
        Fields,
        Parameters,
    ],
)]
impl SpellingModule {}

async fn status(server: &TestServer, path: &str, header: Option<&str>) -> u16 {
    let mut request = server.client().get(server.url(path));
    if let Some(header) = header {
        request = request.header(header, "1");
    }
    request.send().await.unwrap().status().as_u16()
}

#[tokio::test]
async fn every_key_spelling_fills_its_field() {
    let server = TestServer::start(SpellingModule).await;

    let body = server
        .client()
        .get(server.url("/spell/keys"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    assert_eq!(body, "ulo 8080 alpha beta tagged 8080");
}

#[tokio::test]
async fn every_guard_spelling_reaches_its_guard() {
    let server = TestServer::start(SpellingModule).await;

    let cases = [
        ("/spell/type", "x-plain"),
        ("/spell/qualified-type", "x-qualified"),
        ("/spell/capitals-type", "x-http"),
        ("/spell/qualified-marker", "x-auth"),
        ("/spell/bare-marker", "x-local"),
        ("/spell/generic", "x-tagged"),
        ("/spell/turbofish", "x-tagged"),
        ("/spell/const-value", "x-admin"),
        ("/spell/static-value", "x-static"),
    ];
    for (path, header) in cases {
        assert_eq!(status(&server, path, None).await, 403, "{path} refuses");
        assert_eq!(
            status(&server, path, Some(header)).await,
            200,
            "{path} admits `{header}`"
        );
    }
}

#[tokio::test]
async fn a_generic_type_with_two_arguments_is_one_argument() {
    let server = TestServer::start(SpellingModule).await;

    let path = "/spell/two-arguments";
    assert_eq!(
        status(&server, path, Some("x-pair")).await,
        403,
        "both guards run"
    );
    assert_eq!(
        status(&server, path, Some("x-admin")).await,
        403,
        "both guards run"
    );
    let admitted = server
        .client()
        .get(server.url(path))
        .header("x-pair", "1")
        .header("x-admin", "1")
        .send()
        .await
        .unwrap();
    assert_eq!(admitted.status(), 200);
}

#[tokio::test]
async fn a_const_interceptor_and_error_handler_are_values() {
    let server = TestServer::start(SpellingModule).await;

    let stamped = server
        .client()
        .get(server.url("/spell/const-interceptor"))
        .send()
        .await
        .unwrap();
    assert_eq!(stamped.headers()["x-stamp"], "1");

    assert_eq!(
        status(&server, "/spell/const-error-handler", None).await,
        418
    );
}

#[tokio::test]
async fn a_static_interceptor_and_error_handler_are_taken_by_reference() {
    let server = TestServer::start(SpellingModule).await;

    let stamped = server
        .client()
        .get(server.url("/spell/static-interceptor"))
        .send()
        .await
        .unwrap();
    assert_eq!(stamped.headers()["x-stamp"], "1");

    assert_eq!(
        status(&server, "/spell/static-error-handler", None).await,
        418
    );
}
