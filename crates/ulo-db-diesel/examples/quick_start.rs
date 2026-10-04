//! A Diesel connection pool, injected by type.
//!
//! `DieselModule::postgres` registers a deadpool `Pool<AsyncPgConnection>`. A
//! connection is checked out per query and returned on drop, which is why the
//! service holds the pool rather than a connection.
//!
//! Diesel supports postgres and mysql here; sqlite has no async driver in this
//! integration.
//!
//!     DATABASE_URL=postgres://postgres:postgres@localhost/postgres \
//!         cargo run -p ulo-db-diesel --features postgres --example quick_start
//!     curl http://127.0.0.1:3000/health/db

use diesel_async::AsyncPgConnection;
use diesel_async::pooled_connection::deadpool::Pool;
use ulo::http::Body;
use ulo::{UloFactory, controller, get, injectable, module, routes};
use ulo_db_diesel::DieselModule;
use ulo_http_hyper::AxumAdapter;

#[injectable]
pub struct DbProbe {
    #[inject]
    pool: Pool<AsyncPgConnection>,
}

impl DbProbe {
    async fn check(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Checking a connection out is the probe: the pool is lazy, so a
        // misconfigured URL surfaces here rather than at startup.
        let _conn = self.pool.get().await?;
        Ok(())
    }
}

#[controller("/health")]
pub struct HealthController {
    #[inject]
    probe: DbProbe,
}

#[routes]
impl HealthController {
    #[get("/db")]
    async fn db(&self) -> Body {
        match self.probe.check().await {
            Ok(()) => Body::text("ok"),
            Err(e) => Body::text(format!("error: {e}")),
        }
    }
}

#[module(imports: [DieselModule::postgres(std::env::var("DATABASE_URL").expect("DATABASE_URL"))], controllers: [HealthController], providers: [DbProbe])]
impl AppModule {}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut app = UloFactory::create(AppModule).await?;
    app.use_http_adapter(AxumAdapter::new(), ("127.0.0.1", 3000))?;
    println!("listening on http://127.0.0.1:3000");
    app.start().await?;
    Ok(())
}
