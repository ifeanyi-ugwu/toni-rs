use crate::connection::SeaOrmConnectionFactory;
use sea_orm::DatabaseConnection;
use ulo::StartupCheck;
use ulo::di::{CheckedModule, DynamicModule, Key, token_of};

pub struct SeaOrmModule;

impl SeaOrmModule {
    /// Register a database connection for the entire application.
    ///
    /// Returns a global `DynamicModule` that provides `DatabaseConnection` to every
    /// module without requiring explicit imports. Import this once in your root module:
    ///
    /// ```ignore
    /// #[module(imports: [SeaOrmModule::for_root(env!("DATABASE_URL"))])]
    /// pub struct AppModule;
    /// ```
    ///
    /// Then inject `DatabaseConnection` anywhere:
    ///
    /// ```ignore
    /// #[injectable]
    /// pub struct UserService {
    ///     #[inject]
    ///     db: DatabaseConnection,
    /// }
    /// impl UserService {
    ///     pub async fn find_all(&self) -> Result<Vec<user::Model>, DbErr> {
    ///         user::Entity::find().all(&self.db).await
    ///     }
    /// }
    /// ```
    pub fn for_root(database_url: impl Into<String>) -> CheckedModule {
        let database_url: String = database_url.into();

        CheckedModule::new(move |check: Option<StartupCheck>| {
            #[allow(unused_mut)]
            let mut builder = DynamicModule::builder("SeaOrmModule")
                .provider_factory(SeaOrmConnectionFactory {
                    database_url: database_url.clone(),
                    token: ulo::di::token_of::<DatabaseConnection>(),
                    check,
                })
                .export::<DatabaseConnection>();

            #[cfg(feature = "health")]
            {
                builder = builder
                    .provider_factory(crate::health::SeaOrmHealthIndicatorFactory)
                    .export::<crate::health::SeaOrmHealthIndicator>();
            }

            builder.global().build()
        })
    }

    /// Register a second database connection, under the marker `K`.
    ///
    /// `for_root` provides one `DatabaseConnection` injectable by type. A second one needs a slot
    /// of its own, named by a marker type whose slot holds a `DatabaseConnection`:
    ///
    /// ```ignore
    /// key!(pub Analytics: DatabaseConnection);
    ///
    /// #[module(imports: [
    ///     SeaOrmModule::for_root(env!("PRIMARY_URL")),
    ///     SeaOrmModule::for_root_keyed::<Analytics>(env!("ANALYTICS_URL")),
    /// ])]
    /// pub struct AppModule;
    ///
    /// #[injectable]
    /// pub struct ReportService {
    ///     #[inject(Analytics)]
    ///     db: DatabaseConnection,
    /// }
    /// ```
    ///
    /// Two connections under one marker are refused at startup, whichever integrations register
    /// them. The connection only is registered — the health indicator is attached to the default
    /// `for_root` connection.
    pub fn for_root_keyed<K>(database_url: impl Into<String>) -> CheckedModule
    where
        K: Key<Value = DatabaseConnection>,
    {
        let database_url: String = database_url.into();

        CheckedModule::new(move |check: Option<StartupCheck>| {
            DynamicModule::builder(token_of::<K>())
                .provider_factory(SeaOrmConnectionFactory {
                    database_url: database_url.clone(),
                    token: token_of::<K>(),
                    check,
                })
                .export::<K>()
                .global()
                .build()
        })
    }
}
