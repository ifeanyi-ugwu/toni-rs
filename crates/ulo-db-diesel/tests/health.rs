#![cfg(feature = "integration")]

use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;

use diesel::{QueryableByName, sql_types::Text};
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;
use ulo::dispatch::{Http, IntoOutput};
use ulo::http::Body;
use ulo::http::extract::Bytes;
use ulo::prelude::*;
use ulo_db_diesel::{AsyncPgConnection, DieselModule, PgHealthIndicator, PgPool};
use ulo_health::{HealthCheckService, HealthIndicator, TerminusModule};
use ulo_http_axum::AxumAdapter;

static DB_URL: OnceLock<String> = OnceLock::new();

#[derive(QueryableByName)]
struct ItemRow {
    #[diesel(sql_type = Text)]
    name: String,
}

#[injectable]
pub struct ItemService {
    #[inject]
    pool: PgPool,
}
impl ItemService {
    async fn setup(&self) {
        use ulo_db_diesel::RunQueryDsl;
        let mut conn = self.pool.get().await.unwrap();
        diesel::sql_query(
            "CREATE TABLE IF NOT EXISTS items (id SERIAL PRIMARY KEY, name TEXT NOT NULL)",
        )
        .execute(&mut *conn)
        .await
        .unwrap();
    }

    async fn insert(&self, name: String) {
        use ulo_db_diesel::RunQueryDsl;
        let mut conn = self.pool.get().await.unwrap();
        diesel::sql_query("INSERT INTO items (name) VALUES ($1)")
            .bind::<Text, _>(name)
            .execute(&mut *conn)
            .await
            .unwrap();
    }

    async fn list(&self) -> Vec<String> {
        use ulo_db_diesel::RunQueryDsl;
        let mut conn = self.pool.get().await.unwrap();
        diesel::sql_query("SELECT name FROM items ORDER BY id")
            .load::<ItemRow>(&mut *conn)
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.name)
            .collect()
    }
}

#[controller("/items")]
pub struct ItemController {
    #[inject]
    service: Arc<ItemService>,
    #[inject]
    health: HealthCheckService,
    #[inject]
    indicator: PgHealthIndicator,
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
    async fn health(&self) -> impl IntoOutput<Http> {
        self.health
            .check(vec![self.indicator.check("database")])
            .await
    }
}

#[module(
    imports: [
        DieselModule::postgres(DB_URL.get().expect("DB_URL not set").clone()),
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

    let app_port: u16 = 19084;

    tokio::spawn(async move {
        let mut app = UloFactory::new().create_with(TestModule).await.unwrap();
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
}
