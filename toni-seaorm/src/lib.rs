mod connection;
#[cfg(feature = "health")]
pub mod health;
mod module;

pub use module::SeaOrmModule;

#[cfg(feature = "health")]
pub use health::SeaOrmHealthIndicator;
pub use sea_orm::{ActiveModelTrait, DatabaseConnection, DbErr, EntityTrait, Set};
