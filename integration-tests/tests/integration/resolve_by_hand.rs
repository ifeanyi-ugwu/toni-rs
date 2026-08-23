//! Resolving a provider by hand, outside any handler.
//!
//! A request-scoped provider is built into an execution's cache, so resolving one
//! means having an execution to resolve it in — a transport's context where the
//! work arrived over a wire, a standalone execution where it did not.

use std::collections::HashMap;

use toni::context::{HttpContext, RpcContext};
use toni::http_helpers::RequestPart;
use toni::toni_factory::ToniFactory;
use toni::{injectable, module, new, ProviderContext, RpcData};
use uuid::Uuid;

#[derive(Debug)]
#[injectable(scope = "request")]
pub struct Stamp {
    pub id: String,
}

impl Stamp {
    #[new]
    pub fn new() -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
        }
    }
}

#[module(providers: [Stamp])]
impl TestModule {}

fn request_parts() -> RequestPart {
    http::Request::builder().body(()).unwrap().into_parts().0
}

#[tokio::test]
async fn one_execution_holds_one_instance() {
    let app = ToniFactory::create(TestModule).await;
    let execution = ProviderContext::standalone();

    let first = app
        .resolve::<Stamp>(&execution)
        .await
        .expect("a request-scoped provider resolves in an execution");
    let second = app
        .resolve::<Stamp>(&execution)
        .await
        .expect("a request-scoped provider resolves in an execution");

    assert_eq!(
        first.id, second.id,
        "the second resolution finds the instance the first built"
    );
}

#[tokio::test]
async fn a_second_execution_builds_its_own() {
    let app = ToniFactory::create(TestModule).await;

    let first = app
        .resolve::<Stamp>(&ProviderContext::standalone())
        .await
        .expect("a request-scoped provider resolves in an execution");
    let second = app
        .resolve::<Stamp>(&ProviderContext::standalone())
        .await
        .expect("a request-scoped provider resolves in an execution");

    assert_ne!(first.id, second.id);
}

#[tokio::test]
async fn a_transport_execution_resolves_the_same_way() {
    let app = ToniFactory::create(TestModule).await;

    let http: ProviderContext = HttpContext::from_parts(request_parts()).into();
    let first = app
        .resolve::<Stamp>(&http)
        .await
        .expect("an HTTP request is an execution");
    let second = app
        .resolve::<Stamp>(&http)
        .await
        .expect("an HTTP request is an execution");
    assert_eq!(first.id, second.id);

    let rpc: ProviderContext = RpcContext::new(
        "orders.create",
        RpcData::json(serde_json::json!({})),
        HashMap::new(),
        None,
    )
    .into();
    let third = app
        .resolve::<Stamp>(&rpc)
        .await
        .expect("an RPC call is an execution");

    assert_ne!(
        first.id, third.id,
        "two executions, whatever they arrived over"
    );
}

#[tokio::test]
async fn without_an_execution_there_is_nothing_to_resolve_in() {
    let app = ToniFactory::create(TestModule).await;

    let error = app
        .resolve::<Stamp>(&ProviderContext::None)
        .await
        .expect_err("`None` is the absence of an execution, not one to build in")
        .to_string();

    assert!(
        error.contains("Stamp") && error.contains("request-scoped"),
        "the refusal should name the provider and its scope, got: {error}"
    );
}
