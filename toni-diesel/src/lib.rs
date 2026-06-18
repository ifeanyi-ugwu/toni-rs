#[cfg(feature = "health")]
pub mod health;
mod module;
mod pool;

pub use module::DieselModule;

pub use diesel::{prelude, result::Error as DieselError};

#[cfg(any(feature = "mysql", feature = "postgres"))]
pub use diesel_async::RunQueryDsl;

#[cfg(feature = "mysql")]
pub use diesel_async::AsyncMysqlConnection;
#[cfg(feature = "mysql")]
pub type MySqlPool =
    diesel_async::pooled_connection::deadpool::Pool<diesel_async::AsyncMysqlConnection>;

#[cfg(feature = "postgres")]
pub use diesel_async::AsyncPgConnection;
#[cfg(feature = "postgres")]
pub type PgPool = diesel_async::pooled_connection::deadpool::Pool<diesel_async::AsyncPgConnection>;

#[cfg(all(feature = "health", feature = "mysql"))]
pub use health::MySqlHealthIndicator;
#[cfg(all(feature = "health", feature = "postgres"))]
pub use health::PgHealthIndicator;
