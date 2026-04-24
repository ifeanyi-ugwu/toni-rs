use mongodb::Database;
use toni::DynamicModule;

use crate::connection::MongoConnectionFactory;

pub struct MongoModule;

impl MongoModule {
    /// Register a MongoDB database for the entire application.
    ///
    /// Returns a global `DynamicModule` that provides `Database` to every module
    /// without requiring explicit imports. Import this once in your root module:
    ///
    /// ```ignore
    /// #[module(imports: [MongoModule::for_root(env!("MONGODB_URI"), "my_db")])]
    /// pub struct AppModule;
    /// ```
    ///
    /// Then inject `Database` anywhere:
    ///
    /// ```ignore
    /// #[injectable(pub struct UserService {
    ///     db: Database,
    /// })]
    /// impl UserService {
    ///     pub async fn find_all(&self) -> Result<Vec<User>, mongodb::error::Error> {
    ///         let col = self.db.collection::<User>("users");
    ///         col.find(doc! {}).await?.try_collect().await
    ///     }
    /// }
    /// ```
    pub fn for_root(uri: impl Into<String>, db_name: impl Into<String>) -> DynamicModule {
        let uri: String = uri.into();
        let db_name: String = db_name.into();

        #[allow(unused_mut)]
        let mut builder = DynamicModule::builder("MongoModule")
            .provider(MongoConnectionFactory {
                uri: uri.clone(),
                db_name: db_name.clone(),
            })
            .export::<Database>();

        #[cfg(feature = "health")]
        {
            builder = builder
                .provider(crate::health::MongoHealthIndicatorFactory { uri, db_name })
                .export::<crate::health::MongoHealthIndicator>();
        }

        builder.global().build()
    }
}
