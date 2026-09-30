//! A sqlx pool, injected by type, and a second one injected through a marker.
//!
//! `SqlxModule::postgres` registers `Pool<Postgres>` under its own type. A
//! second pool of the same type cannot be told apart by type alone, so it is
//! registered under a marker type and injected with `#[inject(Analytics)]` —
//! the rule every integration here follows for more than one connection.
//!
//!     DATABASE_URL=postgres://postgres:postgres@localhost/postgres \
//!     ANALYTICS_URL=postgres://postgres:postgres@localhost/postgres \
//!         cargo run -p ulo-db-sqlx --features postgres --example quick_start
//!     curl http://127.0.0.1:3000/health/db

use sqlx::{Pool, Postgres};
use std::sync::Arc;
use ulo::http::Body;
use ulo::{UloFactory, controller, get, injectable, key, module, routes};
use ulo_db_sqlx::SqlxModule;
use ulo_http_axum::AxumAdapter;

key!(pub Analytics: Pool<Postgres>);

#[injectable]
pub struct Reports {
    #[inject]
    primary: Pool<Postgres>,
    #[inject(Analytics)]
    analytics: Pool<Postgres>,
}

impl Reports {
    async fn counts(&self) -> Result<(i64, i64), sqlx::Error> {
        let a: (i64,) = sqlx::query_as("SELECT 1").fetch_one(&self.primary).await?;
        let b: (i64,) = sqlx::query_as("SELECT 2")
            .fetch_one(&self.analytics)
            .await?;
        Ok((a.0, b.0))
    }
}

#[controller("/health")]
pub struct HealthController {
    #[inject]
    reports: Arc<Reports>,
}

#[routes]
impl HealthController {
    #[get("/db")]
    async fn db(&self) -> Body {
        match self.reports.counts().await {
            Ok((a, b)) => Body::text(format!("primary={a} analytics={b}")),
            Err(e) => Body::text(format!("error: {e}")),
        }
    }
}

#[module(imports: [SqlxModule::postgres(std::env::var("DATABASE_URL").expect("DATABASE_URL")),
    SqlxModule::postgres_keyed::<Analytics>(std::env::var("ANALYTICS_URL").expect("ANALYTICS_URL"))], controllers: [HealthController], providers: [Reports])]
impl AppModule {}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut app = UloFactory::create(AppModule).await?;
    app.use_http_adapter(AxumAdapter::new(), ("127.0.0.1", 3000))?;
    println!("listening on http://127.0.0.1:3000");
    app.start().await?;
    Ok(())
}
