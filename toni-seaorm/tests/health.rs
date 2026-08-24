#![cfg(feature = "integration")]

use std::sync::OnceLock;
use std::time::Duration;

use sea_orm::{ConnectionTrait, DbBackend, Statement, Value};
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;
use toni::extractors::Bytes;
use toni::*;
use toni_axum::AxumAdapter;
use toni_seaorm::{DatabaseConnection, SeaOrmHealthIndicator, SeaOrmModule};
use toni_terminus::{HealthCheckService, HealthIndicator, TerminusModule};

static DB_URL: OnceLock<String> = OnceLock::new();

#[injectable]
pub struct ItemService {
    #[inject]
    db: DatabaseConnection,
}
impl ItemService {
    async fn setup(&self) {
        self.db
            .execute(Statement::from_string(
                DbBackend::Postgres,
                "CREATE TABLE IF NOT EXISTS items (id SERIAL PRIMARY KEY, name TEXT NOT NULL)"
                    .to_owned(),
            ))
            .await
            .unwrap();
    }

    async fn insert(&self, name: String) {
        self.db
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "INSERT INTO items (name) VALUES ($1)",
                [Value::String(Some(Box::new(name)))],
            ))
            .await
            .unwrap();
    }

    async fn list(&self) -> Vec<String> {
        self.db
            .query_all(Statement::from_string(
                DbBackend::Postgres,
                "SELECT name FROM items ORDER BY id".to_owned(),
            ))
            .await
            .unwrap()
            .iter()
            .map(|r| r.try_get::<String>("", "name").unwrap())
            .collect()
    }
}

#[controller("/items")]
pub struct ItemController {
    #[inject]
    service: ItemService,
    #[inject]
    health: HealthCheckService,
    #[inject]
    indicator: SeaOrmHealthIndicator,
}

#[routes]
impl ItemController {
    #[post("/setup")]
    async fn setup(&self) -> Body {
        self.service.setup().await;
        Body::text("ok".to_string())
    }

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
            .check(vec![self.indicator.check("database")])
            .await
    }
}

#[module(
    imports: [
        SeaOrmModule::for_root(DB_URL.get().expect("DB_URL not set").clone()),
        TerminusModule::new(),
    ],
    controllers: [ItemController],
    providers: [ItemService],
)]
impl TestModule {}

#[tokio::test]
async fn full_integration() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");

    DB_URL.set(url).ok();

    let app_port: u16 = 19083;

    tokio::task::LocalSet::new()
        .run_until(async move {
            tokio::task::spawn_local(async move {
                let mut app = ToniFactory::new().create_with(TestModule).await.unwrap();
                app.use_http_adapter(AxumAdapter::new(), ("127.0.0.1", app_port))
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

            // Create table
            let r = client.post(format!("{base}/setup")).send().await.unwrap();
            assert_eq!(r.status().as_u16(), 200);

            // Write a row
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
            assert_eq!(body["info"]["database"]["status"], "up");
        })
        .await;
}
