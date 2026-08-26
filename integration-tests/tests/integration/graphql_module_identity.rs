//! GraphQL module identity is the full type plus a fingerprint of the value
//! config. Two imports differing in either are distinct modules: different
//! paths mount as two endpoints, colliding on the path is a bind error, and
//! only an identical import dedups as a diamond.

use serial_test::serial;
use toni::toni_factory::ToniFactory;
use toni::{async_trait, module, RequestPart};
use toni_async_graphql::async_graphql::{Data, EmptyMutation, EmptySubscription, Object, Schema};
use toni_async_graphql::{ContextBuilder, DefaultContextBuilder, GraphQLModule};
use toni_axum::AxumAdapter;

use crate::common::TestServer;

#[derive(Clone)]
struct Query;

#[Object]
impl Query {
    async fn ping(&self) -> &'static str {
        "pong"
    }
}

fn gql(
    path: &str,
) -> GraphQLModule<Query, EmptyMutation, EmptySubscription, DefaultContextBuilder> {
    let schema = Schema::build(Query, EmptyMutation, EmptySubscription).finish();
    GraphQLModule::for_root(schema, DefaultContextBuilder).with_path(path)
}

async fn ping(server: &TestServer, path: &str) -> serde_json::Value {
    let resp = server
        .client()
        .post(server.url(path))
        .json(&serde_json::json!({"query": "{ ping }"}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        200,
        "POST {path} must reach a mounted endpoint"
    );
    resp.json().await.unwrap()
}

#[module(imports: [gql("/gql-a"), gql("/gql-b")])]
impl TwoPathsModule {}

/// Same schema at two paths is two modules, and both serve.
#[serial]
#[tokio_localset_test::localset_test]
async fn two_paths_mount_two_endpoints() {
    let server = TestServer::start(TwoPathsModule).await;

    assert_eq!(ping(&server, "/gql-a").await["data"]["ping"], "pong");
    assert_eq!(ping(&server, "/gql-b").await["data"]["ping"], "pong");
}

struct OtherContext;

#[async_trait]
impl ContextBuilder for OtherContext {
    async fn build(&self, _req: &RequestPart) -> Data {
        Data::default()
    }
}

#[module(imports: [
    gql("/gql-clash"),
    GraphQLModule::for_root(
        Schema::build(Query, EmptyMutation, EmptySubscription).finish(),
        OtherContext,
    ).with_path("/gql-clash"),
])]
impl ClashModule {}

/// A different context builder at the same path is a distinct module, and the
/// collision is refused instead of one builder silently winning. The refusal
/// is the native router's duplicate-route unwind (ADR-0011); that it panics
/// out of `bind` instead of failing it is a tracked gap.
#[serial]
#[test]
fn a_second_context_builder_on_one_path_is_refused() {
    let message = crate::common::panic_message(|| async {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let mut app = ToniFactory::create(ClashModule).await.unwrap();
                app.use_http_adapter(AxumAdapter::new(), ("127.0.0.1", 0))
                    .unwrap();
                let _ = app.bind().await;
            })
            .await;
    });

    assert!(
        message.contains("Overlapping method route"),
        "expected the duplicate-route refusal, got:\n{message}"
    );
}

#[module(imports: [gql("/gql-a"), gql("/gql-a")])]
impl DiamondModule {}

/// An identical import is still a diamond: one module, one endpoint.
#[serial]
#[tokio_localset_test::localset_test]
async fn an_identical_import_dedups() {
    let server = TestServer::start(DiamondModule).await;

    assert_eq!(ping(&server, "/gql-a").await["data"]["ping"], "pong");
}
