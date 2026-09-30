//! A handler without `&self` is routed like any other, and may sit in the same
//! impl block as handlers that take one.
//!
//! The macro decides per method whether to emit an instance receiver, so a
//! mixed block is where that decision is observable. Execution scope is covered
//! because a static handler still runs inside an execution even though it holds
//! no instance to scope.
use crate::common::TestServer;
use std::sync::Arc;
use ulo::http::{Body, HttpRequest};
use ulo::{controller, get, injectable, module, routes};
#[controller("/static")]
pub struct StaticController {}

#[routes]
impl StaticController {
    #[get("/hello")]
    fn hello(_req: HttpRequest) -> Body {
        Body::text("Hello from static method".to_string())
    }

    #[get("/world")]
    fn world(_req: HttpRequest) -> Body {
        Body::text("World from static method".to_string())
    }
}

#[module(controllers: [StaticController], providers: [])]
impl StaticModule {}

#[tokio::test]
async fn static_method_controller() {
    let server = TestServer::start(StaticModule).await;

    let resp = server
        .client()
        .get(server.url("/static/hello"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "Hello from static method");

    let resp = server
        .client()
        .get(server.url("/static/world"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "World from static method");
}

#[injectable]
pub struct MixedService {}
impl MixedService {
    pub fn get_instance_message(&self) -> String {
        "From instance method".to_string()
    }
}

#[controller("/mixed")]
pub struct MixedController {
    #[inject]
    service: Arc<MixedService>,
}

#[routes]
impl MixedController {
    #[get("/instance")]
    fn instance_method(&self) -> Body {
        Body::text(self.service.get_instance_message())
    }

    #[get("/static")]
    fn static_method(_req: HttpRequest) -> Body {
        Body::text("From static method".to_string())
    }
}

#[module(controllers: [MixedController], providers: [MixedService])]
impl MixedModule {}

#[tokio::test]
async fn mixed_static_and_instance_methods() {
    let server = TestServer::start(MixedModule).await;

    let resp = server
        .client()
        .get(server.url("/mixed/instance"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "From instance method");

    let resp = server
        .client()
        .get(server.url("/mixed/static"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "From static method");
}

#[controller("/request-static", scope = "execution")]
pub struct RequestScopedStaticController {}

#[routes]
impl RequestScopedStaticController {
    #[get("/test")]
    fn test(_req: HttpRequest) -> Body {
        Body::text("Static method in execution-scoped controller".to_string())
    }
}

#[module(controllers: [RequestScopedStaticController], providers: [])]
impl RequestScopedStaticModule {}

#[tokio::test]
async fn request_scoped_static_methods() {
    let server = TestServer::start(RequestScopedStaticModule).await;

    let resp = server
        .client()
        .get(server.url("/request-static/test"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.text().await.unwrap(),
        "Static method in execution-scoped controller"
    );
}

#[controller("/async-static")]
pub struct AsyncStaticController {}

#[routes]
impl AsyncStaticController {
    #[get("/test")]
    async fn test(_req: HttpRequest) -> Body {
        tokio::time::sleep(tokio::time::Duration::from_millis(1)).await;
        Body::text("Async static method".to_string())
    }
}

#[module(controllers: [AsyncStaticController], providers: [])]
impl AsyncStaticModule {}

#[tokio::test]
async fn async_static_methods() {
    let server = TestServer::start(AsyncStaticModule).await;

    let resp = server
        .client()
        .get(server.url("/async-static/test"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "Async static method");
}
