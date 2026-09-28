# ulo-db-mongodb

MongoDB integration for the [Ulo framework](https://github.com/ulo-rs/ulo).

Registers a `mongodb::Database` in Ulo's DI container so any injectable can declare it as a dependency. The underlying connection pool is shut down cleanly on application shutdown.

## Installation

```toml
[dependencies]
ulo-db-mongodb = "0.1"
```

## Setup

Import `MongoModule::for_root` once in your root module. `Database` becomes available to every module in the application without further imports.

```rust
use ulo_db_mongodb::MongoModule;

#[module(imports: [MongoModule::for_root(env!("MONGODB_URI"), "my_db")])]
pub struct AppModule;
```

## Injecting the database

Declare `Database` as a field in any injectable and work with collections directly:

```rust
use ulo_db_mongodb::{Database, Collection, doc, MongoError};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
struct User {
    name: String,
    email: String,
}

#[injectable]
pub struct UserService {
    #[inject]
    db: Database,
}

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

`ulo-db-mongodb` re-exports the types you'll use most (`Database`, `Collection`, `Document`, `doc!`, `ObjectId`, `FindOptions`, `MongoError`), so in most cases you only need to depend on `ulo-db-mongodb`.

## Multiple databases

`for_root` registers `Database` under its own type. A second database needs a slot of its own, named by a marker type holding a `Database`, and is registered with `MongoModule::for_root_keyed::<K>(uri, db_name)` and injected with `#[inject(K)]`:

```rust,ignore
key!(pub Analytics: Database);

#[module(imports: [
    MongoModule::for_root(env!("PRIMARY_URI"), "primary"),
    MongoModule::for_root_keyed::<Analytics>(env!("ANALYTICS_URI"), "analytics"),
])]
pub struct AppModule;
```

## License

MIT
