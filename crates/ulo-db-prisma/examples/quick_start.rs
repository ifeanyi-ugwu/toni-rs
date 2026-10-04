//! A Prisma client, injected by its generated type.
//!
//! `PrismaModule::for_root` takes a closure producing the client that
//! `cargo prisma generate` wrote, and registers it under that concrete type.
//! Unlike the other five integrations there is no startup check: the client is
//! an opaque generated type with no operation the framework can call to see
//! whether it works, so an unreachable database surfaces at first use.
//!
//! The stand-in below is what the generated `db::PrismaClient` occupies in a
//! real application — this example compiles and runs without a schema.
//!
//!     cargo run -p ulo-db-prisma --example quick_start
//!     curl http://127.0.0.1:3000/users

use ulo::http::Body;
use ulo::{UloFactory, controller, get, injectable, module, routes};
use ulo_db_prisma::PrismaModule;
use ulo_http_hyper::AxumAdapter;

/// Stands in for the generated `db::PrismaClient`.
#[derive(Clone)]
pub struct PrismaClient {
    url: String,
}

impl PrismaClient {
    async fn user_count(&self) -> usize {
        // `self.user().find_many(vec![]).exec().await` in a generated client.
        self.url.len()
    }
}

#[injectable]
pub struct Users {
    #[inject]
    db: PrismaClient,
}

#[controller("/users")]
pub struct UsersController {
    #[inject]
    users: Users,
}

#[routes]
impl UsersController {
    #[get("/")]
    async fn count(&self) -> Body {
        Body::json(serde_json::json!({ "count": self.users.db.user_count().await }))
    }
}

#[module(imports: [PrismaModule::for_root(|| async { PrismaClient { url: "postgres://localhost/app".into() } })], controllers: [UsersController], providers: [Users])]
impl AppModule {}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut app = UloFactory::create(AppModule).await?;
    app.use_http_adapter(AxumAdapter::new(), ("127.0.0.1", 3000))?;
    println!("listening on http://127.0.0.1:3000");
    app.start().await?;
    Ok(())
}
