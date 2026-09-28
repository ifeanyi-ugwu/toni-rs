# ulo-db-seaorm

SeaORM integration for the [Ulo framework](https://github.com/ulo-rs/ulo).

Registers a `DatabaseConnection` in Ulo's DI container so any injectable can declare it as a dependency. The connection is backed by a SeaORM connection pool and closed cleanly on application shutdown.

## Installation

```toml
[dependencies]
ulo-db-seaorm = { version = "0.1", features = ["sqlx-postgres", "runtime-tokio-rustls"] }
```

Pick one backend feature and one runtime:

| Backend         | Feature         |
| --------------- | --------------- |
| PostgreSQL      | `sqlx-postgres` |
| MySQL / MariaDB | `sqlx-mysql`    |
| SQLite          | `sqlx-sqlite`   |

| Runtime                  | Feature                    |
| ------------------------ | -------------------------- |
| Tokio + rustls (default) | `runtime-tokio-rustls`     |
| Tokio + native-tls       | `runtime-tokio-native-tls` |

## Setup

Import `SeaOrmModule::for_root` once in your root module. That's it — `DatabaseConnection` becomes available to every module in the application without further imports.

```rust
use ulo_db_seaorm::SeaOrmModule;

#[module(imports: [SeaOrmModule::for_root(env!("DATABASE_URL"))])]
pub struct AppModule;
```

## Injecting the connection

Declare `DatabaseConnection` as a field in any injectable:

```rust
use ulo_db_seaorm::{DatabaseConnection, DbErr, EntityTrait};

#[injectable]
pub struct UserService {
    #[inject]
    db: DatabaseConnection,
}

impl UserService {
    pub async fn find_all(&self) -> Result<Vec<user::Model>, DbErr> {
        user::Entity::find().all(&self.db).await
    }

    pub async fn find_by_id(&self, id: i32) -> Result<Option<user::Model>, DbErr> {
        user::Entity::find_by_id(id).one(&self.db).await
    }
}
```

`ulo-db-seaorm` re-exports the SeaORM types you'll use most (`DatabaseConnection`, `DbErr`, `EntityTrait`, `ActiveModelTrait`, `Set`), so in most cases you only need to depend on `ulo-db-seaorm`.

## Repository pattern

SeaORM's API is entity-centric (`user::Entity::find().all(&db)`) rather than repository-centric, so `ulo-db-seaorm` does not provide a `Repository<E>` wrapper — it would only obscure the query builder. If you want a repository layer, write one with `#[injectable]`:

```rust
#[injectable]
pub struct UserRepository {
    #[inject]
    db: DatabaseConnection,
}

impl UserRepository {
    pub async fn find_active(&self) -> Result<Vec<user::Model>, DbErr> {
        user::Entity::find()
            .filter(user::Column::IsActive.eq(true))
            .all(&self.db)
            .await
    }
}
```

## Multiple databases

`for_root` registers `DatabaseConnection` under its own type. A second connection needs a slot of its own, named by a marker type holding a `DatabaseConnection`, and is registered with `SeaOrmModule::for_root_keyed::<K>(url)` and injected with `#[inject(K)]`:

```rust,ignore
key!(pub Analytics: DatabaseConnection);

#[module(imports: [
    SeaOrmModule::for_root(env!("PRIMARY_URL")),
    SeaOrmModule::for_root_keyed::<Analytics>(env!("ANALYTICS_URL")),
])]
pub struct AppModule;
```

## License

MIT
