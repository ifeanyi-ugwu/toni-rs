# toni-mongodb

MongoDB integration for the [Toni framework](https://github.com/monterxto/toni-rs).

Registers a `mongodb::Database` in Toni's DI container so any injectable can declare it as a dependency. The underlying connection pool is shut down cleanly on application shutdown.

## Installation

```toml
[dependencies]
toni-mongodb = "0.1"
```

## Setup

Import `MongoModule::for_root` once in your root module. `Database` becomes available to every module in the application without further imports.

```rust
use toni_mongodb::MongoModule;

#[module(imports: [MongoModule::for_root(env!("MONGODB_URI"), "my_db")])]
pub struct AppModule;
```

## Injecting the database

Declare `Database` as a field in any injectable and work with collections directly:

```rust
use toni_mongodb::{Database, Collection, doc, MongoError};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
struct User {
    name: String,
    email: String,
}

#[injectable(pub struct UserService {
    db: Database,
})]
impl UserService {
    pub async fn find_all(&self) -> Result<Vec<User>, MongoError> {
        let col: Collection<User> = self.db.collection("users");
        col.find(doc! {}).await?.try_collect().await
    }

    pub async fn find_by_email(&self, email: &str) -> Result<Option<User>, MongoError> {
        let col: Collection<User> = self.db.collection("users");
        col.find_one(doc! { "email": email }).await
    }
}
```

`toni-mongodb` re-exports the types you'll use most (`Database`, `Collection`, `Document`, `doc!`, `ObjectId`, `FindOptions`, `MongoError`), so in most cases you only need to depend on `toni-mongodb`.

## Multiple databases

`for_root` registers `Database` under the `mongodb::Database` type token. Calling it twice overwrites the first registration. If you need multiple databases, inject `Database` and call `client.database("other_db")` — or open an issue.

## License

MIT
