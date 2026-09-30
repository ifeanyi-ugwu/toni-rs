//! A SeaORM connection, injected by type.
//!
//! `SeaOrmModule::for_root` registers a `DatabaseConnection` under its own
//! type, so a service takes it with a bare `#[inject]` and no import of the
//! module. Construction is lazy; the module's startup check is what proves the
//! server answers, and it turns an unreachable database into a startup failure
//! rather than a 500 on the first request.
//!
//!     DATABASE_URL=postgres://postgres:postgres@localhost/postgres \
//!         cargo run -p ulo-db-seaorm --example quick_start
//!     curl http://127.0.0.1:3000/health/db

use sea_orm::{ConnectionTrait, DatabaseConnection, Statement};
use std::sync::Arc;
use ulo::http::Body;
use ulo::{UloFactory, controller, get, injectable, module, routes};
use ulo_db_seaorm::SeaOrmModule;
use ulo_http_axum::AxumAdapter;

#[injectable]
pub struct DbProbe {
    #[inject]
    db: DatabaseConnection,
}

impl DbProbe {
    /// The smallest query that proves the connection round-trips.
    async fn now(&self) -> Result<String, sea_orm::DbErr> {
        let backend = self.db.get_database_backend();
        let row = self
            .db
            .query_one(Statement::from_string(backend, "SELECT 1 AS one"))
            .await?;
        Ok(match row {
            Some(r) => format!("{}", r.try_get::<i32>("", "one")?),
            None => "no rows".to_string(),
        })
    }
}

#[controller("/health")]
pub struct HealthController {
    #[inject]
    probe: Arc<DbProbe>,
}

#[routes]
impl HealthController {
    #[get("/db")]
    async fn db(&self) -> Body {
        match self.probe.now().await {
            Ok(v) => Body::text(format!("ok: {v}")),
            Err(e) => Body::text(format!("error: {e}")),
        }
    }
}

#[module(imports: [SeaOrmModule::for_root(std::env::var("DATABASE_URL").expect("DATABASE_URL"))], controllers: [HealthController], providers: [DbProbe])]
impl AppModule {}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut app = UloFactory::create(AppModule).await?;
    app.use_http_adapter(AxumAdapter::new(), ("127.0.0.1", 3000))?;
    println!("listening on http://127.0.0.1:3000");
    app.start().await?;
    Ok(())
}
