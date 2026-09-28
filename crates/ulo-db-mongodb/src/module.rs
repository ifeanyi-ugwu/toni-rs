use crate::connection::MongoConnectionFactory;
use mongodb::Database;
use ulo::StartupCheck;
use ulo::di::{CheckedModule, DynamicModule, Key, token_of};

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
    pub fn for_root(uri: impl Into<String>, db_name: impl Into<String>) -> CheckedModule {
        let uri: String = uri.into();
        let db_name: String = db_name.into();

        CheckedModule::new(move |check: Option<StartupCheck>| {
            #[allow(unused_mut)]
            let mut builder = DynamicModule::builder("MongoModule")
                .provider(MongoConnectionFactory {
                    uri: uri.clone(),
                    db_name: db_name.clone(),
                    token: ulo::di::token_of::<Database>(),
                    check,
                })
                .export::<Database>();

            #[cfg(feature = "health")]
            {
                builder = builder
                    .provider(crate::health::MongoHealthIndicatorFactory)
                    .export::<crate::health::MongoHealthIndicator>();
            }

            builder.global().build()
        })
    }

    /// Register a second MongoDB database, under the marker `K`.
    ///
    /// `for_root` provides one `Database` injectable by type. A second one needs a slot of its own,
    /// named by a marker type whose slot holds a `Database`:
    ///
    /// ```ignore
    /// key!(pub Analytics: Database);
    ///
    /// #[module(imports: [
    ///     MongoModule::for_root(env!("PRIMARY_URI"), "primary"),
    ///     MongoModule::for_root_keyed::<Analytics>(env!("ANALYTICS_URI"), "analytics"),
    /// ])]
    /// pub struct AppModule;
    ///
    /// #[injectable]
    /// pub struct ReportService {
    ///     #[inject(Analytics)]
    ///     db: Database,
    /// }
    /// ```
    ///
    /// Two databases under one marker are refused at startup, whichever integrations register them.
    /// The database only is registered — the health indicator is attached to the default `for_root`
    /// database.
    pub fn for_root_keyed<K>(uri: impl Into<String>, db_name: impl Into<String>) -> CheckedModule
    where
        K: Key<Value = Database>,
    {
        let uri: String = uri.into();
        let db_name: String = db_name.into();
        CheckedModule::new(move |check: Option<StartupCheck>| {
            DynamicModule::builder(token_of::<K>())
                .provider(MongoConnectionFactory {
                    uri: uri.clone(),
                    db_name: db_name.clone(),
                    token: token_of::<K>(),
                    check,
                })
                .export::<K>()
                .global()
                .build()
        })
    }
}
