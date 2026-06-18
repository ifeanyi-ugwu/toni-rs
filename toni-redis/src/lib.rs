mod connection;
#[cfg(feature = "health")]
pub mod health;
mod module;

pub use module::RedisModule;

#[cfg(feature = "health")]
pub use health::RedisHealthIndicator;
pub use redis::{AsyncCommands, RedisError, RedisResult, aio::ConnectionManager};
