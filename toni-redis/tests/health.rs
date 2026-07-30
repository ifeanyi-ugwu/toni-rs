#![cfg(feature = "integration")]

use std::sync::OnceLock;
use std::time::Duration;

use testcontainers::runners::AsyncRunner;
use testcontainers_modules::redis::Redis;
use toni::extractors::{Bytes, Path};
use toni::*;
use toni_axum::AxumAdapter;
use toni_redis::{AsyncCommands, ConnectionManager, RedisHealthIndicator, RedisModule};
use toni_terminus::{HealthCheckService, HealthIndicator, TerminusModule};

static DB_URL: OnceLock<String> = OnceLock::new();

#[injectable]
pub struct CacheService {
    #[inject]
    manager: ConnectionManager,
}
impl CacheService {
    async fn set(&self, key: &str, value: &str) {
        let mut conn = self.manager.clone();
        let _: () = conn.set(key, value).await.unwrap();
    }

    async fn get(&self, key: &str) -> Option<String> {
        let mut conn = self.manager.clone();
        conn.get(key).await.unwrap()
    }
}

#[controller("/cache")]
pub struct CacheController {
    #[inject]
    service: CacheService,
    #[inject]
    health: HealthCheckService,
    #[inject]
    indicator: RedisHealthIndicator,
}

#[routes]
impl CacheController {
    #[post("/")]
    async fn set(&self, Bytes(body): Bytes) -> Body {
        // body is "key=value"
        let s = String::from_utf8_lossy(&body).into_owned();
        let (key, value) = s.split_once('=').unwrap();
        self.service.set(key, value).await;
        Body::text("ok".to_string())
    }

    #[get("/{key}")]
    async fn get(&self, Path(key): Path<String>) -> Body {
        let value = self.service.get(&key).await.unwrap_or_default();
        Body::text(value)
    }

    #[get("/health")]
    async fn health(&self) -> impl IntoResponse {
        self.health.check(vec![self.indicator.check("redis")]).await
    }
}

#[module(
    imports: [
        RedisModule::for_root(DB_URL.get().expect("DB_URL not set").clone()),
        TerminusModule::new(),
    ],
    controllers: [CacheController],
    providers: [CacheService],
)]
impl TestModule {}

#[tokio::test]
async fn full_integration() {
    let container = Redis::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(6379).await.unwrap();
    let url = format!("redis://127.0.0.1:{port}");

    DB_URL.set(url).ok();

    let app_port: u16 = 19081;

    tokio::task::LocalSet::new()
        .run_until(async move {
            tokio::task::spawn_local(async move {
                let mut app = ToniFactory::new().create_with(TestModule).await;
                app.use_http_adapter(AxumAdapter::new(), ("127.0.0.1", app_port))
                    .unwrap();
                app.start().await.unwrap();
            });

            let base = format!("http://127.0.0.1:{app_port}/cache");
            let client = reqwest::Client::new();
            for _ in 0..20u8 {
                if client.get(format!("{base}/health")).send().await.is_ok() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(300)).await;
            }

            // Write a key
            let r = client
                .post(&base)
                .body("greeting=hello")
                .send()
                .await
                .unwrap();
            assert_eq!(r.status().as_u16(), 200);

            // Read it back
            let r = client.get(format!("{base}/greeting")).send().await.unwrap();
            let status = r.status().as_u16();
            let body_text = r.text().await.unwrap();
            assert_eq!(
                status, 200,
                "GET /cache/greeting failed ({status}): {body_text}"
            );

            // Health check
            let r = client.get(format!("{base}/health")).send().await.unwrap();
            assert_eq!(r.status().as_u16(), 200);
            let body: serde_json::Value = r.json().await.unwrap();
            assert_eq!(body["status"], "ok");
            assert_eq!(body["info"]["redis"]["status"], "up");
        })
        .await;
}
