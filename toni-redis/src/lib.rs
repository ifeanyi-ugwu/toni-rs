mod connection;
mod module;
#[cfg(feature = "health")]
pub mod health;

pub use module::RedisModule;

pub use redis::{AsyncCommands, RedisError, RedisResult, aio::ConnectionManager};
#[cfg(feature = "health")]
pub use health::RedisHealthIndicator;
