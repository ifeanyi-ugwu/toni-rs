//! Execution-scoped instances stay isolated when requests overlap in time.
//!
//! A scope bug is invisible sequentially — one request at a time gets a correct
//! instance whether or not the cache is per-request — and surfaces only under
//! concurrency, as one request reading another's state.
// provider_scope.rs proves that sequential requests get isolated execution-scoped
// instances. This file proves the same holds under concurrency: if the request
// context machinery has any shared mutable state (a stray RefCell, a map keyed
// on thread-id instead of request-id), concurrent requests would see each
// other's instances and this test would catch it.

use crate::common::TestServer;
use futures_util::future::join_all;
use std::sync::Arc;
use ulo::http::Body;
use ulo::{controller, get, module, provide, routes};
use uuid::Uuid;

#[tokio::test]
async fn request_scoped_instances_are_isolated_under_concurrency() {
    ulo::key!(ReqId: RequestId);
    #[derive(Clone)]
    struct RequestId {
        id: String,
    }

    #[controller("/", scope = "execution")]
    pub struct TestController {
        #[inject(ReqId)]
        req_id: Arc<RequestId>,
    }

    #[routes]
    impl TestController {
        #[get("/id")]
        fn get_id(&self) -> Body {
            Body::text(self.req_id.id.clone())
        }
    }

    #[module(
        controllers: [TestController],
        providers: [
            provide!(ReqId => async || RequestId { id: Uuid::new_v4().to_string() }).per_execution(),
        ],
    )]
    impl TestModule {}

    let server = TestServer::start(TestModule).await;

    const N: usize = 20;
    let futs: Vec<_> = (0..N)
        .map(|_| {
            let client = server.client().clone();
            let url = server.url("/id");
            async move { client.get(url).send().await.unwrap().text().await.unwrap() }
        })
        .collect();

    let ids = join_all(futs).await;

    let unique: std::collections::HashSet<_> = ids.iter().collect();
    assert_eq!(
        unique.len(),
        N,
        "each of the {} concurrent requests must get a distinct execution-scoped ID; \
        duplicates indicate scope context leak: {:?}",
        N,
        ids
    );
}
