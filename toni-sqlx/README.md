# toni-sqlx

SQLx integration for the [Toni framework](https://github.com/monterxto/toni-rs).

Registers a SQLx connection pool in Toni's DI container so any injectable can declare it as a dependency. The pool is closed cleanly on application shutdown.

## Installation

```toml
[dependencies]
toni-sqlx = { version = "0.1", features = ["postgres", "runtime-tokio-rustls"] }
```

Pick one backend feature and one runtime:

| Backend         | Feature    |
| --------------- | ---------- |
| PostgreSQL      | `postgres` |
| MySQL / MariaDB | `mysql`    |
| SQLite          | `sqlite`   |

| Runtime                  | Feature                    |
| ------------------------ | -------------------------- |
| Tokio + rustls (default) | `runtime-tokio-rustls`     |
| Tokio + native-tls       | `runtime-tokio-native-tls` |

## Setup

Import the appropriate constructor once in your root module. The pool becomes available to every module in the application without further imports.

```rust
use toni_sqlx::SqlxModule;

// PostgreSQL
#[module(imports: [SqlxModule::postgres(env!("DATABASE_URL"))])]
pub struct AppModule;

// MySQL
#[module(imports: [SqlxModule::mysql(env!("DATABASE_URL"))])]
pub struct AppModule;

// SQLite
#[module(imports: [SqlxModule::sqlite(env!("DATABASE_URL"))])]
pub struct AppModule;
```

## Injecting the pool

Declare the pool as a field in any injectable:

```rust
use sqlx::{PgPool, Row};
use toni_sqlx::SqlxError;

#[injectable]
pub struct UserService {
    #[inject]
    pool: PgPool,
}

impl UserService {
    pub async fn find_all(&self) -> Result<Vec<User>, SqlxError> {
        sqlx::query_as!(User, "SELECT id, name FROM users")
            .fetch_all(&self.pool)
            .await
    }

    pub async fn find_by_id(&self, id: i32) -> Result<Option<User>, SqlxError> {
        sqlx::query_as!(User, "SELECT id, name FROM users WHERE id = $1", id)
            .fetch_optional(&self.pool)
            .await
    }
}
```

`PgPool`, `MySqlPool`, and `SqlitePool` are type aliases for `Pool<Postgres>`, `Pool<MySql>`, and `Pool<Sqlite>` respectively — use whichever matches your enabled feature.

## Multiple databases

Each constructor registers its pool under its concrete type token (`Pool<Postgres>`, `Pool<MySql>`, `Pool<Sqlite>`). You can import multiple constructors if you need more than one database — the tokens are distinct, so there is no conflict.

```rust
#[module(imports: [
    SqlxModule::postgres(env!("PRIMARY_DB_URL")),
    SqlxModule::sqlite(env!("CACHE_DB_URL")),
])]
pub struct AppModule;
```

Then inject `PgPool` and `SqlitePool` as separate fields in the same service.

## License

MIT
