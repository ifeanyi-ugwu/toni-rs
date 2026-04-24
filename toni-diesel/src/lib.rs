mod module;
mod pool;
#[cfg(feature = "health")]
pub mod health;

pub use module::DieselModule;

pub use diesel::{prelude, result::Error as DieselError};

#[cfg(feature = "mysql")]
pub use diesel_async::{
    AsyncMysqlConnection, RunQueryDsl, pooled_connection::deadpool::Pool as MySqlPool,
};
#[cfg(feature = "postgres")]
pub use diesel_async::{
    AsyncPgConnection, RunQueryDsl, pooled_connection::deadpool::Pool as PgPool,
};

#[cfg(all(feature = "health", feature = "postgres"))]
pub use health::PgHealthIndicator;
#[cfg(all(feature = "health", feature = "mysql"))]
pub use health::MySqlHealthIndicator;
