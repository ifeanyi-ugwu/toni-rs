//! A handler may be `async fn`, and the dispatcher awaits it before writing the
//! response.
//!
//! The macro generates the same dispatch arm either way, so a sync handler
//! passing says nothing about an async one: the failure mode is a response
//! written from a future nobody polled.
use crate::common::TestServer;
use std::sync::Arc;
use ulo::http::Body;
use ulo::{controller, get, injectable, module, routes};
#[injectable]
pub struct AsyncService;
impl AsyncService {
    pub async fn fetch_data(&self) -> String {
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        "async data".to_string()
    }

    pub async fn compute(&self, value: i32) -> i32 {
        tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;
        value * 2
    }
}

#[controller("/async")]
pub struct AsyncController {
    #[inject]
    service: Arc<AsyncService>,
}

#[routes]
impl AsyncController {
    #[get("/data")]
    async fn get_data(&self) -> Body {
        Body::text(self.service.fetch_data().await)
    }

    #[get("/compute")]
    async fn compute(&self) -> Body {
        Body::text(format!("Result: {}", self.service.compute(42).await))
    }

    #[get("/sync")]
    fn sync_method(&self) -> Body {
        Body::text("sync response".to_string())
    }

    #[get("/multi")]
    async fn multi_await(&self) -> Body {
        let data = self.service.fetch_data().await;
        let result = self.service.compute(10).await;
        Body::text(format!("{} - {}", data, result))
    }
}

#[module(controllers: [AsyncController], providers: [AsyncService])]
impl AsyncModule {}

#[tokio::test]
async fn async_controller_methods() {
    let server = TestServer::start(AsyncModule).await;

    let resp = server
        .client()
        .get(server.url("/async/data"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "async data");

    let resp = server
        .client()
        .get(server.url("/async/compute"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "Result: 84");

    let resp = server
        .client()
        .get(server.url("/async/sync"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "sync response");

    let resp = server
        .client()
        .get(server.url("/async/multi"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "async data - 20");
}
