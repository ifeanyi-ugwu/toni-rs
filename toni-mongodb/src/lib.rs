mod connection;
mod module;
#[cfg(feature = "health")]
pub mod health;

pub use module::MongoModule;
#[cfg(feature = "health")]
pub use health::MongoHealthIndicator;

pub use mongodb::{
    Collection, Database,
    bson::{Document, doc, oid::ObjectId},
    error::Error as MongoError,
    options::FindOptions,
};
