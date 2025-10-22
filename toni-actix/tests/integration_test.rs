use toni::{
    controller, controller_struct, get, module, post, provider_struct, Body as ToniBody,
    HttpAdapter, HttpRequest,
};
use toni_actix::ActixAdapter;

// Simple service for testing
#[provider_struct(
    pub struct TestService;
)]
impl TestService {
    pub fn get_greeting(&self) -> String {
        "Hello from Actix!".to_string()
    }

    pub fn echo(&self, message: String) -> String {
        format!("Echo: {}", message)
    }
}

// Simple controller for testing
#[controller_struct(
    pub struct TestController {
        test_service: TestService,
    }
)]
#[controller("/test")]
impl TestController {
    #[get("/hello")]
    fn hello(&self, _req: HttpRequest) -> ToniBody {
        let response: String = self.test_service.get_greeting();
        ToniBody::Text(response)
    }

    #[post("/echo")]
    fn echo(&self, req: HttpRequest) -> ToniBody {
        let message = match req.body {
            ToniBody::Text(text) => text,
            ToniBody::Json(json) => json.to_string(),
            //_ => "".to_string(),
        };
        let response: String = self.test_service.echo(message);
        ToniBody::Text(response)
    }
}

// Test module
#[module(
    imports: [],
    controllers: [TestController],
    providers: [TestService],
    exports: []
)]
impl TestModule {}

#[actix_rt::test]
async fn test_actix_e2e() {
    use std::time::Duration;
    use toni::toni_factory::ToniFactory;

    let port = 18081;
    let local = tokio::task::LocalSet::new();

    // Spawn server in background
    local.spawn_local(async move {
        let adapter = ActixAdapter::new();
        let factory = ToniFactory::new();
        let app = factory.create(TestModule::module_definition(), adapter);
        let _ = app.listen(port, "127.0.0.1").await;
    });

    // Run tests within the LocalSet
    local
        .run_until(async move {
            // Give the server time to start
            tokio::time::sleep(Duration::from_millis(500)).await;

            let client = reqwest::Client::new();

            // Test GET request
            let get_response = client
                .get(format!("http://127.0.0.1:{}/test/hello", port))
                .send()
                .await
                .expect("GET request failed");

            assert_eq!(get_response.status(), 200);
            assert_eq!(get_response.text().await.unwrap(), "Hello from Actix!");

            // Test POST request
            let post_response = client
                .post(format!("http://127.0.0.1:{}/test/echo", port))
                .body("test message")
                .send()
                .await
                .expect("POST request failed");

            assert_eq!(post_response.status(), 200);
            assert_eq!(post_response.text().await.unwrap(), "Echo: test message");
        })
        .await;
}
