# ulo-db-sqlx

SQLx integration for the [Ulo framework](https://github.com/ulo-rs/ulo).

Registers a SQLx connection pool in Ulo's DI container so any injectable can declare it as a dependency. The pool is closed cleanly on application shutdown.

## Installation

```toml
[dependencies]
ulo-db-sqlx = { version = "0.1", features = ["postgres", "runtime-tokio-rustls"] }
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
use ulo_db_sqlx::SqlxModule;

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
use ulo_db_sqlx::SqlxError;

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

Two pools of one backend share a type, so the second is registered under a marker holding that pool type, with `SqlxModule::postgres_keyed::<K>(url)` (or `mysql_keyed`, `sqlite_keyed`), and injected with `#[inject(K)]`:

```rust,ignore
key!(pub Analytics: Pool<Postgres>);

#[module(imports: [
    SqlxModule::postgres(env!("PRIMARY_DB_URL")),
    SqlxModule::postgres_keyed::<Analytics>(env!("ANALYTICS_DB_URL")),
])]
pub struct AppModule;
```

## License

MIT
