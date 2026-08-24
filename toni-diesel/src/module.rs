#[cfg(any(feature = "postgres", feature = "mysql"))]
use toni::DynamicModule;

pub struct DieselModule;

impl DieselModule {
    #[cfg(feature = "postgres")]
    pub fn postgres(url: impl Into<String>) -> DynamicModule {
        use diesel_async::{AsyncPgConnection, pooled_connection::deadpool::Pool};

        use crate::pool::PgPoolFactory;
        let url: String = url.into();

        #[allow(unused_mut)]
        let mut builder = DynamicModule::builder("DieselModule::postgres")
            .provider(PgPoolFactory {
                url: url.clone(),
                token: std::any::type_name::<Pool<AsyncPgConnection>>().to_string(),
            })
            .export::<Pool<AsyncPgConnection>>();

        #[cfg(feature = "health")]
        {
            builder = builder
                .provider(crate::health::PgHealthIndicatorFactory)
                .export::<crate::health::PgHealthIndicator>();
        }

        builder.global().build()
    }

    #[cfg(feature = "mysql")]
    pub fn mysql(url: impl Into<String>) -> DynamicModule {
        use diesel_async::{AsyncMysqlConnection, pooled_connection::deadpool::Pool};

        use crate::pool::MySqlPoolFactory;
        let url: String = url.into();

        #[allow(unused_mut)]
        let mut builder = DynamicModule::builder("DieselModule::mysql")
            .provider(MySqlPoolFactory {
                url: url.clone(),
                token: std::any::type_name::<Pool<AsyncMysqlConnection>>().to_string(),
            })
            .export::<Pool<AsyncMysqlConnection>>();

        #[cfg(feature = "health")]
        {
            builder = builder
                .provider(crate::health::MySqlHealthIndicatorFactory)
                .export::<crate::health::MySqlHealthIndicator>();
        }

        builder.global().build()
    }

    /// Register a second, named Postgres pool.
    ///
    /// `postgres` provides one `Pool<AsyncPgConnection>` injectable by type. When an application
    /// needs more than one pool, each additional one is registered under a name and injected by
    /// that name — the type alone can no longer tell them apart.
    ///
    /// ```ignore
    /// #[module(imports: [
    ///     DieselModule::postgres(env!("PRIMARY_URL")),
    ///     DieselModule::postgres_named("analytics", env!("ANALYTICS_URL")),
    /// ])]
    /// pub struct AppModule;
    /// ```
    ///
    /// ```ignore
    /// #[injectable]
    /// pub struct ReportService {
    ///     #[inject("analytics")]
    ///     pool: Pool<AsyncPgConnection>,
    /// }
    /// ```
    ///
    /// The name is a global identifier: two pools cannot share one, and reusing a name across
    /// integrations is refused at startup. The pool only is registered — the health indicator is
    /// attached to the default `postgres` pool.
    #[cfg(feature = "postgres")]
    pub fn postgres_named(name: impl Into<String>, url: impl Into<String>) -> DynamicModule {
        use crate::pool::PgPoolFactory;
        let name: String = name.into();
        DynamicModule::builder(format!("DieselModule::postgres::{name}"))
            .provider(PgPoolFactory {
                url: url.into(),
                token: name.clone(),
            })
            .export_token(name)
            .global()
            .build()
    }

    /// Register a second, named MySQL pool, injected by `name` rather than by type.
    #[cfg(feature = "mysql")]
    pub fn mysql_named(name: impl Into<String>, url: impl Into<String>) -> DynamicModule {
        use crate::pool::MySqlPoolFactory;
        let name: String = name.into();
        DynamicModule::builder(format!("DieselModule::mysql::{name}"))
            .provider(MySqlPoolFactory {
                url: url.into(),
                token: name.clone(),
            })
            .export_token(name)
            .global()
            .build()
    }
}
