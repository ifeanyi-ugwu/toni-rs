mod connection;
#[cfg(feature = "health")]
pub mod health;
mod module;

#[cfg(feature = "health")]
pub use health::MongoHealthIndicator;
pub use module::MongoModule;

pub use mongodb::{
    Collection, Database,
    bson::{Document, doc, oid::ObjectId},
    error::Error as MongoError,
    options::FindOptions,
};
