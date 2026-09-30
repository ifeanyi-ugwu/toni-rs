//! A Redis connection manager, injected by type.
//!
//! `RedisModule::for_root` registers `ConnectionManager`, which reconnects on
//! its own, so a service holds it and issues commands without managing the
//! socket. This is the cache-and-keys integration; `ulo-rpc-redis` carries RPC
//! over the same server and `ulo-ws-redis` carries WebSocket broadcast, and
//! the three are separate crates because they are separate concerns.
//!
//!     REDIS_URL=redis://127.0.0.1:6379 \
//!         cargo run -p ulo-db-redis --example quick_start
//!     curl -X POST http://127.0.0.1:3000/counter
//!     curl http://127.0.0.1:3000/counter

use redis::AsyncCommands;
use std::sync::Arc;
use ulo::http::Body;
use ulo::{UloFactory, controller, get, injectable, module, post, routes};
use ulo_db_redis::{ConnectionManager, RedisModule};
use ulo_http_axum::AxumAdapter;

#[injectable]
pub struct Counter {
    #[inject]
    redis: ConnectionManager,
}

impl Counter {
    async fn bump(&self) -> redis::RedisResult<i64> {
        // `ConnectionManager` is cheap to clone and each clone shares the
        // reconnecting connection underneath.
        let mut conn = self.redis.clone();
        conn.incr("ulo:example:counter", 1).await
    }

    async fn read(&self) -> redis::RedisResult<i64> {
        let mut conn = self.redis.clone();
        Ok(conn.get("ulo:example:counter").await.unwrap_or(0))
    }
}

#[controller("/counter")]
pub struct CounterController {
    #[inject]
    counter: Arc<Counter>,
}

#[routes]
impl CounterController {
    #[post("/")]
    async fn bump(&self) -> Body {
        match self.counter.bump().await {
            Ok(v) => Body::text(v.to_string()),
            Err(e) => Body::text(format!("error: {e}")),
        }
    }

    #[get("/")]
    async fn read(&self) -> Body {
        match self.counter.read().await {
            Ok(v) => Body::text(v.to_string()),
            Err(e) => Body::text(format!("error: {e}")),
        }
    }
}

#[module(imports: [RedisModule::for_root(std::env::var("REDIS_URL").expect("REDIS_URL"))], controllers: [CounterController], providers: [Counter])]
impl AppModule {}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut app = UloFactory::create(AppModule).await?;
    app.use_http_adapter(AxumAdapter::new(), ("127.0.0.1", 3000))?;
    println!("listening on http://127.0.0.1:3000");
    app.start().await?;
    Ok(())
}
