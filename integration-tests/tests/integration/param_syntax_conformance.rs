//! Conformance suite for `{param}` route parameters.
//!
//! `{param}` is the canonical parameter syntax (ADR-0012); every HTTP adapter
//! must extract it, and the adapters that answer method mismatches from their
//! own route table (salvo, actix, rocket) must recognize a `{param}` segment
//! when deciding 405 vs 404.

use toni::extractors::Path;
use toni::{controller, get, module, routes, Body as ToniBody, ToniFactory};

use crate::common::TestServer;

#[controller("/users")]
pub struct UsersController {}

#[routes]
impl UsersController {
    #[get("/{id}")]
    fn get_user(&self, Path(id): Path<u32>) -> ToniBody {
        ToniBody::text(format!("user:{id}"))
    }
}

#[module(controllers: [UsersController])]
impl ParamSyntaxModule {}

async fn boot(adapter: impl toni::HttpAdapter + 'static) -> TestServer {
    TestServer::start_adapter(ToniFactory::new(), ParamSyntaxModule, adapter).await
}

async fn case_param_extracted(server: TestServer) {
    let resp = server
        .client()
        .get(server.url("/users/7"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "user:7");
}

async fn case_method_mismatch_on_param_path(server: TestServer) {
    // /users/{id} only has GET — a POST to a matching path is a method
    // mismatch (405), not an unknown path (404).
    let resp = server
        .client()
        .post(server.url("/users/7"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 405);
}

macro_rules! param_syntax_suite {
    ($adapter_mod:ident, $adapter:expr) => {
        mod $adapter_mod {
            #[tokio_localset_test::localset_test]
            async fn param_is_extracted() {
                super::case_param_extracted(super::boot($adapter).await).await;
            }

            #[tokio_localset_test::localset_test]
            async fn method_mismatch_on_param_path_is_405() {
                super::case_method_mismatch_on_param_path(super::boot($adapter).await).await;
            }
        }
    };
}

param_syntax_suite!(axum, toni_axum::AxumAdapter::new());
param_syntax_suite!(poem, toni_poem::PoemAdapter::new());
param_syntax_suite!(salvo, toni_salvo::SalvoAdapter::new());
param_syntax_suite!(actix, toni_actix::ActixAdapter::new());
param_syntax_suite!(rocket, toni_rocket::RocketAdapter::new());
