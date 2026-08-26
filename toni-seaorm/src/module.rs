use sea_orm::DatabaseConnection;
use toni::{CheckedModule, DynamicModule, StartupCheck};

use crate::connection::SeaOrmConnectionFactory;

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
                .provider(SeaOrmConnectionFactory {
                    database_url: database_url.clone(),
                    token: toni::di::token_of::<DatabaseConnection>(),
                    check,
                })
                .export::<DatabaseConnection>();

            #[cfg(feature = "health")]
            {
                builder = builder
                    .provider(crate::health::SeaOrmHealthIndicatorFactory)
                    .export::<crate::health::SeaOrmHealthIndicator>();
            }

            builder.global().build()
        })
    }

    /// Register a second, named database connection.
    ///
    /// `for_root` provides one `DatabaseConnection` injectable by type. When an application needs
    /// more than one connection, each additional one is registered under a name and injected by
    /// that name — the type alone can no longer tell them apart.
    ///
    /// ```ignore
    /// #[module(imports: [
    ///     SeaOrmModule::for_root(env!("PRIMARY_URL")),
    ///     SeaOrmModule::for_root_named("analytics", env!("ANALYTICS_URL")),
    /// ])]
    /// pub struct AppModule;
    /// ```
    ///
    /// ```ignore
    /// #[injectable]
    /// pub struct ReportService {
    ///     #[inject("analytics")]
    ///     db: DatabaseConnection,
    /// }
    /// ```
    ///
    /// The name is a global identifier: two connections cannot share one, and reusing a name
    /// across integrations is refused at startup. The connection only is registered — the health
    /// indicator is attached to the default `for_root` connection.
    pub fn for_root_named(
        name: impl Into<String>,
        database_url: impl Into<String>,
    ) -> CheckedModule {
        let name: String = name.into();
        let database_url: String = database_url.into();

        CheckedModule::new(move |check: Option<StartupCheck>| {
            DynamicModule::builder(format!("SeaOrmModule::{name}"))
                .provider(SeaOrmConnectionFactory {
                    database_url: database_url.clone(),
                    token: name.clone(),
                    check,
                })
                .export_token(name.clone())
                .global()
                .build()
        })
    }
}
