# toni-diesel

Diesel async integration for the [Toni framework](https://github.com/monterxto/toni-rs).

Registers a `diesel-async` deadpool connection pool in Toni's DI container so any injectable can declare it as a dependency. The pool is closed cleanly on application shutdown.

## Installation

```toml
[dependencies]
toni-diesel = { version = "0.1", features = ["postgres"] }
```

Pick one backend feature:

| Backend         | Feature    |
| --------------- | ---------- |
| PostgreSQL      | `postgres` |
| MySQL / MariaDB | `mysql`    |

> **MySQL note:** the `mysql` feature uses [`mysql_async`](https://crates.io/crates/mysql_async) under the hood — pure Rust, no system library required. MySQL-specific SQL types (the `diesel::mysql` backend marker) are not enabled by default to avoid pulling in `libmysqlclient`. If you need them, add `diesel = { version = "2", features = ["mysql"] }` to your own `Cargo.toml`.

## Setup

Import the appropriate constructor once in your root module. The pool becomes available to every module in the application without further imports.

```rust
use toni_diesel::DieselModule;

// PostgreSQL
#[module(imports: [DieselModule::postgres(env!("DATABASE_URL"))])]
pub struct AppModule;

// MySQL
#[module(imports: [DieselModule::mysql(env!("DATABASE_URL"))])]
pub struct AppModule;
```

## Injecting the pool

Declare the pool as a field in any injectable and acquire connections with `.get().await`:

```rust
use toni_diesel::{DieselError, PgPool, RunQueryDsl, prelude::*};

#[injectable]
pub struct UserService {
    #[inject]
    pool: PgPool,
}

impl UserService {
    pub async fn find_all(&self) -> Result<Vec<User>, DieselError> {
        use crate::schema::users::dsl::*;
        let mut conn = self.pool.get().await.unwrap();
        users.load::<User>(&mut conn).await
    }
}
```

## Schema

`toni-diesel` does not run migrations. Use the Diesel CLI:

```sh
diesel setup
diesel migration generate create_users
diesel migration run
```

## License

MIT
