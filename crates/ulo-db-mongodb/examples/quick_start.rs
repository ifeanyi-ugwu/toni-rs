//! A MongoDB database handle, injected by type.
//!
//! `MongoModule::for_root` takes the URI and the database name, and registers
//! the resulting `Database`. Collections are typed at the call site, so the
//! handle is all a service needs.
//!
//!     MONGODB_URI=mongodb://127.0.0.1:27017 \
//!         cargo run -p ulo-db-mongodb --example quick_start
//!     curl http://127.0.0.1:3000/users

use futures::TryStreamExt;
use mongodb::Database;
use mongodb::bson::doc;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use ulo::http::Body;
use ulo::{UloFactory, controller, get, injectable, module, post, routes};
use ulo_db_mongodb::MongoModule;
use ulo_http_axum::AxumAdapter;

#[derive(Debug, Serialize, Deserialize)]
pub struct User {
    name: String,
}

#[injectable]
pub struct Users {
    #[inject]
    db: Database,
}

impl Users {
    async fn add(&self, name: &str) -> mongodb::error::Result<()> {
        self.db
            .collection::<User>("users")
            .insert_one(User { name: name.into() })
            .await?;
        Ok(())
    }

    async fn all(&self) -> mongodb::error::Result<Vec<User>> {
        self.db
            .collection::<User>("users")
            .find(doc! {})
            .await?
            .try_collect()
            .await
    }
}

#[controller("/users")]
pub struct UsersController {
    #[inject]
    users: Arc<Users>,
}

#[routes]
impl UsersController {
    #[post("/")]
    async fn add(&self) -> Body {
        match self.users.add("ada").await {
            Ok(()) => Body::text("inserted"),
            Err(e) => Body::text(format!("error: {e}")),
        }
    }

    #[get("/")]
    async fn all(&self) -> Body {
        match self.users.all().await {
            Ok(users) => Body::json(serde_json::json!({ "count": users.len() })),
            Err(e) => Body::text(format!("error: {e}")),
        }
    }
}

#[module(imports: [MongoModule::for_root(std::env::var("MONGODB_URI").expect("MONGODB_URI"), "ulo_example")], controllers: [UsersController], providers: [Users])]
impl AppModule {}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut app = UloFactory::create(AppModule).await?;
    app.use_http_adapter(AxumAdapter::new(), ("127.0.0.1", 3000))?;
    println!("listening on http://127.0.0.1:3000");
    app.start().await?;
    Ok(())
}
