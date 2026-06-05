#![cfg(feature = "integration")]

use std::sync::OnceLock;
use std::time::Duration;

use futures::TryStreamExt;
use mongodb::bson::{Document, doc};
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::mongo::Mongo;
use toni::extractors::Bytes;
use toni::*;
use toni_axum::AxumAdapter;
use toni_mongodb::{Database, MongoHealthIndicator, MongoModule};
use toni_terminus::{HealthCheckService, HealthIndicator, TerminusModule};

static DB_URL: OnceLock<String> = OnceLock::new();

#[injectable]
pub struct ItemService {
    #[inject]
    db: Database,
}
impl ItemService {
    async fn insert(&self, name: String) {
        self.db
            .collection::<Document>("items")
            .insert_one(doc! { "name": name })
            .await
            .unwrap();
    }

    async fn list(&self) -> Vec<String> {
        self.db
            .collection::<Document>("items")
            .find(doc! {})
            .await
            .unwrap()
            .try_collect::<Vec<Document>>()
            .await
            .unwrap()
            .into_iter()
            .filter_map(|d| d.get_str("name").ok().map(str::to_string))
            .collect()
    }
}

#[controller(
    "/items",
    pub struct ItemController {
        #[inject]
        service: ItemService,
        #[inject]
        health: HealthCheckService,
        #[inject]
        indicator: MongoHealthIndicator,
    }
)]
impl ItemController {
    #[post("/")]
    async fn create(&self, Bytes(body): Bytes) -> Body {
        let name = String::from_utf8_lossy(&body).into_owned();
        self.service.insert(name).await;
        Body::text("ok".to_string())
    }

    #[get("/")]
    async fn list(&self) -> Body {
        Body::json(serde_json::json!(self.service.list().await))
    }

    #[get("/health")]
    async fn health(&self) -> impl IntoResponse {
        self.health
            .check(vec![self.indicator.check("mongodb")])
            .await
    }
}

#[module(
    imports: [
        MongoModule::for_root(
            DB_URL.get().expect("DB_URL not set").clone(),
            "testdb",
        ),
        TerminusModule::new(),
    ],
    controllers: [ItemController],
    providers: [ItemService],
)]
impl TestModule {}

#[tokio::test]
async fn full_integration() {
    let container = Mongo::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(27017).await.unwrap();
    let uri = format!("mongodb://127.0.0.1:{port}");

    DB_URL.set(uri).ok();

    let app_port: u16 = 19085;

    tokio::task::LocalSet::new()
        .run_until(async move {
            tokio::task::spawn_local(async move {
                let mut app = ToniFactory::new()
                    .create_with(TestModule::module_definition())
                    .await;
                app.use_http_adapter(AxumAdapter::new(), app_port, "127.0.0.1")
                    .unwrap();
                app.start().await.unwrap();
            });

            let base = format!("http://127.0.0.1:{app_port}/items");
            let client = reqwest::Client::new();
            for _ in 0..20u8 {
                if client.get(format!("{base}/health")).send().await.is_ok() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(300)).await;
            }

            // Write a document
            let r = client.post(&base).body("hello").send().await.unwrap();
            assert_eq!(r.status().as_u16(), 200);

            // Read it back
            let r = client.get(&base).send().await.unwrap();
            assert_eq!(r.status().as_u16(), 200);
            let items: Vec<String> = r.json().await.unwrap();
            assert_eq!(items, vec!["hello"]);

            // Health check
            let r = client.get(format!("{base}/health")).send().await.unwrap();
            assert_eq!(r.status().as_u16(), 200);
            let body: serde_json::Value = r.json().await.unwrap();
            assert_eq!(body["status"], "ok");
            assert_eq!(body["info"]["mongodb"]["status"], "up");
        })
        .await;
}
