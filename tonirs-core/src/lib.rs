use linkme::distributed_slice;

#[path = "adapter/http_adapter.rs"]
pub mod http_adapter;
#[path = "adapter/mod.rs"]
pub mod adapter;

// #[path = "common/structs/mod.rs"]
// pub mod common_structs;
pub mod http_helpers;
pub mod traits_helpers;
mod injector;
mod scanner;
pub mod module_helpers;
mod toni_application;
pub mod toni_factory;
mod router;

#[distributed_slice]
pub static FIELDS_STRUCT_CONTROLLER: [(&str, &str)];

#[cfg(test)]
mod tests {
    // use axum_adapter::HttpAdapter;

    use adapter::{AxumAdapter, RouteAdapter};
    use traits_helpers::{Controller, ControllerTrait, ModuleMetadata, Provider, ProviderTrait};
    use http_helpers::{HttpMethod, HttpRequest, HttpResponse};
    use http_adapter::HttpAdapter;
    use rustc_hash::FxHashMap;
    use serde_json::json;
    use std::{any::Any, sync::Arc, time::Duration};
    // use structs::ModuleDefinition;
    use toni_factory::ToniFactory;
    use uuid::Uuid;
    use tokio::task::JoinHandle;

    use super::*;

    // #[insert_struct(
    //     struct UserControlleras<'a> {
    //         texto: &'a str
    //     }
    // )]
    // #[controllers("/")]
    // impl<'a> UserControlleras<'a> {
    //     #[get("name")]
    //     fn get_name(&self) -> &'static str {
    //         // &self.user_service.get_name("id")
    //         "asd"
    //     }

    //     #[get("idade")]
    //     fn get_idade(&self) -> &'static str {
    //         "32"
    //     }
    // }
    #[tokio::test]
    async fn test_scan() {
        let factory: ToniFactory = ToniFactory;
        let mut adapter: AxumAdapter = AxumAdapter::new();
        // let adapter = MyHttpAdapter;
        // where
        //     F: Fn(Request<Body>) -> Fut + Clone + Send + 'static,
        //     Fut: Future<Output = Response<Body>> + Send,
        // ToniFactory::create::<i32,AxumAdapter>;
        // let module_metadata2 = ModuleMetadata {
        //     id: Uuid::new_v4(),
        //     name: "UserModule".to_string(),
        //     imports: Some(vec![]),
        //     controllers: Some(vec![]),
        //     providers: Some(vec![]),
        //     exports: Some(vec![]),
        // };
        // let module_metadata = ModuleMetadata {
        //     id: Uuid::new_v4(),
        //     name: "AppModule".to_string(),
        //     imports: Some(vec![module_metadata2]),
        //     controllers: Some(vec![]),
        //     providers: Some(vec![]),
        //     exports: Some(vec![]),
        // };
        // let app_module = ModuleDefinition::DefaultModule(&module_metadata);
        // factory.create(app_module, adapter).await;
    }

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
            Ok(res) => {
                // println!("{:?}", res);
                res
            }
            Err(e) => panic!("{}", e),
        };
        println!("{:?}", res);
        let body = match res.json::<serde_json::Value>().await {
            Ok(json) => json,
            Err(e) => panic!("{}", e),
        };
        println!("{:?}", body);
        assert_eq!(body["message"].as_str().unwrap(), "John Doe");
        // let response = client
        //     .post("http://localhost:3000/hello2")
        //     .body(r#"{"uepa": 1}"#)
        //     .send()
        //     .await;
        // let res = match response {
        //     Ok(res) => {
        //         println!("{:?}", res);
        //         res
        //     }
        //     Err(e) => panic!(),
        // };
        // let body = match res.json::<serde_json::Value>().await {
        //     Ok(json) => json,
        //     Err(_) => todo!(),
        // };
        // println!("{:?}", body);
        // assert_eq!(body["message"].as_str().unwrap(), "sei la bixo");

        // Encerra o servidor (opcional)
        server_handle.abort();
    }
}
