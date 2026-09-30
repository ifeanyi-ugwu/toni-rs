//! Provider scope behavior tests
//!
//! When multiple fields inject the same provider token, deduplication behavior depends on scope:
//! - Singleton: Same instance app-wide, persists across requests
//! - Request: Same instance within one execution, fresh instance for the next
//! - Transient: Fresh instance per injection point at construction time

use std::sync::Arc;
use ulo::http::Body;
use ulo::{controller, get, module, provide, routes};
use uuid::Uuid;

use crate::common::TestServer;

#[tokio::test]
async fn scope_behavior() {
    ulo::key!(Singleton: Counter);
    ulo::key!(Request: Counter);
    ulo::key!(Transient: Counter);
    #[derive(Clone)]
    struct Counter {
        id: String,
    }

    impl Counter {
        fn new_singleton() -> Self {
            Self {
                id: Uuid::new_v4().to_string(),
            }
        }

        fn new_request() -> Self {
            Self {
                id: Uuid::new_v4().to_string(),
            }
        }

        fn new_transient() -> Self {
            Self {
                id: Uuid::new_v4().to_string(),
            }
        }
    }

    #[controller("/execution-scoped", scope = "execution")]
    pub struct RequestController {
        #[inject(Singleton)]
        singleton1: Arc<Counter>,
        #[inject(Singleton)]
        singleton2: Arc<Counter>,
        #[inject(Request)]
        request1: Arc<Counter>,
        #[inject(Request)]
        request2: Arc<Counter>,
        #[inject(Transient)]
        transient1: Counter,
        #[inject(Transient)]
        transient2: Counter,
    }

    #[routes]
    impl RequestController {
        #[get("/get")]
        fn get_value(&self) -> Body {
            Body::text(format!(
                "s:{}|{};r:{}|{};t:{}|{}",
                self.singleton1.id,
                self.singleton2.id,
                self.request1.id,
                self.request2.id,
                self.transient1.id,
                self.transient2.id
            ))
        }
    }

    #[controller("/singleton-scoped")]
    pub struct SingletonController {
        #[inject(Transient)]
        transient1: Counter,
        #[inject(Transient)]
        transient2: Counter,
    }

    #[routes]
    impl SingletonController {
        #[get("/get")]
        fn get_value(&self) -> Body {
            Body::text(format!("t:{}|{}", self.transient1.id, self.transient2.id))
        }
    }

    #[module(
        controllers: [RequestController, SingletonController],
        providers: [
            provide!(Singleton => async || Counter::new_singleton()),
            provide!(Request => async || Counter::new_request()).per_execution(),
            provide!(Transient => async || Counter::new_transient()).transient(),
        ],
    )]
    impl TestModule {}

    let server = TestServer::start(TestModule).await;

    let resp1 = server
        .client()
        .get(server.url("/singleton-scoped/get"))
        .send()
        .await
        .unwrap();
    let singleton_body1 = resp1.text().await.unwrap();

    let singleton_ids1: Vec<&str> = singleton_body1
        .trim_start_matches("t:")
        .split('|')
        .collect();
    assert_ne!(
        singleton_ids1[0], singleton_ids1[1],
        "Transient should create different instances per injection point: {}",
        singleton_body1
    );

    let resp2 = server
        .client()
        .get(server.url("/singleton-scoped/get"))
        .send()
        .await
        .unwrap();
    let singleton_body2 = resp2.text().await.unwrap();

    assert_eq!(
        singleton_body1, singleton_body2,
        "Singleton controller should reuse same transient instances across requests: req1={}, req2={}",
        singleton_body1, singleton_body2
    );

    let req_resp1 = server
        .client()
        .get(server.url("/execution-scoped/get"))
        .send()
        .await
        .unwrap();
    let req_body1 = req_resp1.text().await.unwrap();

    let parts1: Vec<&str> = req_body1.split(';').collect();
    let singleton_part1 = parts1[0].trim_start_matches("s:");
    let request_part1 = parts1[1].trim_start_matches("r:");
    let transient_part1 = parts1[2].trim_start_matches("t:");

    let s_ids1: Vec<&str> = singleton_part1.split('|').collect();
    assert_eq!(
        s_ids1[0], s_ids1[1],
        "Singleton should share same instance across injection points: {}",
        req_body1
    );

    let r_ids1: Vec<&str> = request_part1.split('|').collect();
    assert_eq!(
        r_ids1[0], r_ids1[1],
        "Execution scope should share same instance within request: {}",
        req_body1
    );

    let t_ids1: Vec<&str> = transient_part1.split('|').collect();
    assert_ne!(
        t_ids1[0], t_ids1[1],
        "Transient should create different instances per injection point: {}",
        req_body1
    );

    let req_resp2 = server
        .client()
        .get(server.url("/execution-scoped/get"))
        .send()
        .await
        .unwrap();
    let req_body2 = req_resp2.text().await.unwrap();

    let parts2: Vec<&str> = req_body2.split(';').collect();
    let singleton_part2 = parts2[0].trim_start_matches("s:");
    let request_part2 = parts2[1].trim_start_matches("r:");
    let transient_part2 = parts2[2].trim_start_matches("t:");

    let s_ids2: Vec<&str> = singleton_part2.split('|').collect();
    assert_eq!(
        s_ids1[0], s_ids2[0],
        "Singleton should persist same instance across requests: req1={}, req2={}",
        singleton_part1, singleton_part2
    );

    let r_ids2: Vec<&str> = request_part2.split('|').collect();
    assert_ne!(
        r_ids1[0], r_ids2[0],
        "Execution scope should create new instance for new request: req1={}, req2={}",
        request_part1, request_part2
    );

    let t_ids2: Vec<&str> = transient_part2.split('|').collect();
    assert_ne!(
        t_ids1[0], t_ids2[0],
        "Transient should create new instances for new request: req1={}, req2={}",
        transient_part1, transient_part2
    );
}
