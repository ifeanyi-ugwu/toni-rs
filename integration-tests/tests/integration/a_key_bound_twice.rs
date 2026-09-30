//! A key holds one binding, or a collection declared with `into`. A second binding under one key
//! in one module, or a binding beside a collection, fails `create` naming both; every contribution
//! to HTTP's guard collection runs.

use std::sync::Mutex;

use serial_test::serial;
use ulo::enhancer::Guard;
use ulo::http::{Body, HttpContext};
use ulo::{UloFactory, async_trait, controller, get, key, module, provide, routes};

use crate::common::TestServer;

key!(pub Port: u16);

pub trait Plugin: Send + Sync {}

pub struct Alpha;

impl Plugin for Alpha {}

pub struct Beta;

impl Plugin for Beta {}

async fn refusal(module: impl ulo::di::ModuleMetadata + 'static) -> String {
    match UloFactory::create_application_context(module).await {
        Ok(_) => panic!("the module must be refused"),
        Err(err) => err.to_string(),
    }
}

#[module(providers: [provide!(Port => 1u16), provide!(Port => 2u16)])]
struct TwoBindings;

#[module(providers: [provide!(Port => 1u16)])]
struct OneBinding;

#[tokio::test]
async fn a_second_binding_under_one_key_is_refused_naming_both() {
    let err = refusal(TwoBindings).await;
    assert!(err.contains("is bound twice in module"), "{err}");
    assert!(err.contains("entries 1 and 2"), "{err}");

    let ctx = UloFactory::create_application_context(OneBinding)
        .await
        .expect("one binding starts");
    assert_eq!(*ctx.get_key::<Port>().await.unwrap(), 1);
}

#[module(providers: [
    provide!(dyn Plugin => value Alpha),
    provide!(into dyn Plugin => value Beta),
])]
struct BothWaysInOneModule;

#[tokio::test]
async fn a_binding_beside_a_collection_is_refused() {
    let err = refusal(BothWaysInOneModule).await;
    assert!(err.contains("is bound in module"), "{err}");
    assert!(err.contains("collected with `into`"), "{err}");
}

#[module(providers: [provide!(dyn Plugin => value Alpha)])]
struct BindsPlugin;

#[module(providers: [provide!(into dyn Plugin => value Beta)])]
struct CollectsPlugin;

#[module(imports: [BindsPlugin, CollectsPlugin])]
struct BothWaysAcrossModules;

#[tokio::test]
async fn a_binding_and_a_collection_in_two_modules_are_refused_naming_both() {
    let err = refusal(BothWaysAcrossModules).await;
    assert!(err.contains("BindsPlugin"), "{err}");
    assert!(err.contains("CollectsPlugin"), "{err}");
}

static RAN: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

/// Admits every request, recording that it ran.
pub struct Recorder(&'static str);

#[async_trait]
impl Guard<HttpContext> for Recorder {
    async fn can_activate(&self, _ctx: &HttpContext) -> bool {
        RAN.lock().unwrap().push(self.0);
        true
    }
}

#[controller("/guarded")]
pub struct Guarded {}

#[routes]
impl Guarded {
    #[get("/")]
    fn ok(&self) -> Body {
        Body::text("ok".to_string())
    }
}

#[module(
    controllers: [Guarded],
    providers: [
        provide!(into dyn Guard<HttpContext> => value Recorder("first")),
        provide!(into dyn Guard<HttpContext> => value Recorder("second")),
    ],
)]
struct TwoGuardsInOneModule;

#[module(providers: [provide!(into dyn Guard<HttpContext> => value Recorder("first"))])]
struct FirstGuard;

#[module(providers: [provide!(into dyn Guard<HttpContext> => value Recorder("second"))])]
struct SecondGuard;

#[module(imports: [FirstGuard, SecondGuard], controllers: [Guarded])]
struct TwoGuardsInTwoModules;

async fn guards_run_by(module: impl ulo::di::ModuleMetadata + 'static) -> Vec<&'static str> {
    RAN.lock().unwrap().clear();
    let server = TestServer::start(module).await;
    let answer = server
        .client()
        .get(server.url("/guarded"))
        .send()
        .await
        .unwrap();
    assert_eq!(answer.status(), 200);
    RAN.lock().unwrap().clone()
}

#[serial]
#[tokio::test]
async fn two_contributions_to_the_http_guards_both_run_in_declaration_order() {
    assert_eq!(
        guards_run_by(TwoGuardsInOneModule).await,
        ["first", "second"]
    );
}

#[serial]
#[tokio::test]
async fn contributions_from_two_modules_both_run_in_import_order() {
    assert_eq!(
        guards_run_by(TwoGuardsInTwoModules).await,
        ["first", "second"]
    );
}
