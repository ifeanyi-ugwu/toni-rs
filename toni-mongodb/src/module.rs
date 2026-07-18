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
    /// #[injectable]
    /// pub struct UserService {
    ///     #[inject]
    ///     db: Database,
    /// }
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
                token: std::any::type_name::<Database>().to_string(),
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

    /// Register a second, named MongoDB database.
    ///
    /// `for_root` provides one `Database` injectable by type. When an application needs more than
    /// one database, each additional one is registered under a name and injected by that name — the
    /// type alone can no longer tell them apart.
    ///
    /// ```ignore
    /// #[module(imports: [
    ///     MongoModule::for_root(env!("PRIMARY_URI"), "primary"),
    ///     MongoModule::for_root_named("analytics", env!("ANALYTICS_URI"), "analytics"),
    /// ])]
    /// pub struct AppModule;
    /// ```
    ///
    /// ```ignore
    /// #[injectable]
    /// pub struct ReportService {
    ///     #[inject("analytics")]
    ///     db: Database,
    /// }
    /// ```
    ///
    /// The name is a global identifier: two databases cannot share one, and reusing a name across
    /// integrations is refused at startup. The connection only is registered — the health indicator
    /// is attached to the default `for_root` database.
    pub fn for_root_named(
        name: impl Into<String>,
        uri: impl Into<String>,
        db_name: impl Into<String>,
    ) -> DynamicModule {
        let name: String = name.into();
        DynamicModule::builder(format!("MongoModule::{name}"))
            .provider(MongoConnectionFactory {
                uri: uri.into(),
                db_name: db_name.into(),
                token: name.clone(),
            })
            .export_token(name)
            .global()
            .build()
    }
}
