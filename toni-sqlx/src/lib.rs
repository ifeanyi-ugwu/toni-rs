mod redact;

#[cfg(feature = "health")]
pub mod health;
mod module;
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
mod pool;

pub use module::SqlxModule;

#[cfg(feature = "mysql")]
pub use sqlx::{MySqlPool, mysql::MySqlQueryResult, mysql::MySqlRow};
#[cfg(feature = "postgres")]
pub use sqlx::{PgPool, postgres::PgQueryResult, postgres::PgRow};
#[cfg(feature = "sqlite")]
pub use sqlx::{SqlitePool, sqlite::SqliteQueryResult, sqlite::SqliteRow};

pub use sqlx::{Error as SqlxError, Row, query, query_as};

#[cfg(feature = "health")]
pub use health::SqlxHealthIndicator;
#[cfg(all(feature = "health", feature = "postgres"))]
pub type PostgresHealthIndicator = health::SqlxHealthIndicator<sqlx::Postgres>;
#[cfg(all(feature = "health", feature = "mysql"))]
pub type MySqlHealthIndicator = health::SqlxHealthIndicator<sqlx::MySql>;
#[cfg(all(feature = "health", feature = "sqlite"))]
pub type SqliteHealthIndicator = health::SqlxHealthIndicator<sqlx::Sqlite>;
