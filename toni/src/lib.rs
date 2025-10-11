#[path = "adapter/mod.rs"]
pub mod adapter;
#[path = "adapter/http_adapter.rs"]
pub mod http_adapter;
pub mod http_helpers;
pub mod injector;
pub mod module_helpers;
mod router;
mod scanner;
mod structs_helpers;
mod toni_application;
pub mod toni_factory;
pub mod traits_helpers;

#[cfg(test)]
mod tests {
    use std::time::Duration;
    use tokio::task::JoinHandle;

    #[tokio::test]
    async fn test_server() {
        let server_handle: JoinHandle<()> = tokio::spawn(async {
            // let factory = ToniFactory::new();
            // let mut axum_adapter = AxumAdapter::new();
            // let app = factory.create(app_module, axum_adapter).unwrap();
            // app.listen(3000, "127.0.0.1").await;
            // let app = match app {
            //     Ok(app) => {
            //         app
            //     }
            //     Err(e) => panic!("sda")
            // };
            // let axum_adapter2 = AxumAdapter::new();
            // axum_adapter.add_route(&"/ta".to_string(), HttpMethod::GET, Box::new(GetUserNameController));
            // axum_adapter.listen(3000, "127.0.0.1").await;
            // app.listen(3000, "127.0.0.1");
            // servera.get("/ta", |req| Box::pin(route_adapter(req, &Handler)));
            // servera.post("/hello2", |req| Box::pin(route_adapter(req, &Handler2)));
            // servera.listen(3000, "127.0.0.1").await
        });
        tokio::time::sleep(Duration::from_secs(1)).await;
        let client = reqwest::Client::new();
        let response = client.get("http://localhost:3000/names").send().await;
        let res = match response {
            Ok(res) => res,
            Err(e) => panic!("{}", e),
        };

        let body = match res.json::<serde_json::Value>().await {
            Ok(json) => json,
            Err(e) => panic!("{}", e),
        };

        assert_eq!(body["message"].as_str().unwrap(), "John Doe");
        server_handle.abort();
    }
}
