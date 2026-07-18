#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
use std::marker::PhantomData;

#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
use toni::DynamicModule;

#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
use crate::pool::SqlxPoolFactory;

pub struct SqlxModule;

impl SqlxModule {
    #[cfg(feature = "postgres")]
    pub fn postgres(url: impl Into<String>) -> DynamicModule {
        use sqlx::{Pool, Postgres};
        let url: String = url.into();

        #[allow(unused_mut)]
        let mut builder = DynamicModule::builder("SqlxModule::postgres")
            .provider(SqlxPoolFactory::<Postgres> {
                url: url.clone(),
                token: std::any::type_name::<Pool<Postgres>>().to_string(),
                _db: PhantomData,
            })
            .export::<Pool<Postgres>>();

        #[cfg(feature = "health")]
        {
            builder = builder
                .provider(crate::health::SqlxHealthIndicatorFactory::<Postgres> {
                    url,
                    _db: PhantomData,
                })
                .export::<crate::health::SqlxHealthIndicator<Postgres>>();
        }

        builder.global().build()
    }

    /// Register a second, named Postgres pool.
    ///
    /// `postgres` provides one `Pool<Postgres>` injectable by type. When an application needs
    /// more than one pool, each additional one is registered under a name and injected by that
    /// name — the type alone can no longer tell them apart.
    ///
    /// ```ignore
    /// #[module(imports: [
    ///     SqlxModule::postgres(env!("PRIMARY_URL")),
    ///     SqlxModule::postgres_named("analytics", env!("ANALYTICS_URL")),
    /// ])]
    /// pub struct AppModule;
    /// ```
    ///
    /// ```ignore
    /// #[injectable]
    /// pub struct ReportService {
    ///     #[inject("analytics")]
    ///     pool: Pool<Postgres>,
    /// }
    /// ```
    ///
    /// The name is a global identifier: two pools cannot share one, and reusing a name across
    /// integrations is refused at startup. The pool only is registered — the health indicator is
    /// attached to the default `postgres` pool.
    #[cfg(feature = "postgres")]
    pub fn postgres_named(name: impl Into<String>, url: impl Into<String>) -> DynamicModule {
        use sqlx::Postgres;
        let name: String = name.into();
        DynamicModule::builder(format!("SqlxModule::postgres::{name}"))
            .provider(SqlxPoolFactory::<Postgres> {
                url: url.into(),
                token: name.clone(),
                _db: PhantomData,
            })
            .export_token(name)
            .global()
            .build()
    }

    #[cfg(feature = "mysql")]
    pub fn mysql(url: impl Into<String>) -> DynamicModule {
        use sqlx::{MySql, Pool};
        let url: String = url.into();

        #[allow(unused_mut)]
        let mut builder = DynamicModule::builder("SqlxModule::mysql")
            .provider(SqlxPoolFactory::<MySql> {
                url: url.clone(),
                token: std::any::type_name::<Pool<MySql>>().to_string(),
                _db: PhantomData,
            })
            .export::<Pool<MySql>>();

        #[cfg(feature = "health")]
        {
            builder = builder
                .provider(crate::health::SqlxHealthIndicatorFactory::<MySql> {
                    url,
                    _db: PhantomData,
                })
                .export::<crate::health::SqlxHealthIndicator<MySql>>();
        }

        builder.global().build()
    }

    /// Register a second, named MySQL pool. See [`SqlxModule::postgres_named`].
    #[cfg(feature = "mysql")]
    pub fn mysql_named(name: impl Into<String>, url: impl Into<String>) -> DynamicModule {
        use sqlx::MySql;
        let name: String = name.into();
        DynamicModule::builder(format!("SqlxModule::mysql::{name}"))
            .provider(SqlxPoolFactory::<MySql> {
                url: url.into(),
                token: name.clone(),
                _db: PhantomData,
            })
            .export_token(name)
            .global()
            .build()
    }

    #[cfg(feature = "sqlite")]
    pub fn sqlite(url: impl Into<String>) -> DynamicModule {
        use sqlx::{Pool, Sqlite};
        let url: String = url.into();

        #[allow(unused_mut)]
        let mut builder = DynamicModule::builder("SqlxModule::sqlite")
            .provider(SqlxPoolFactory::<Sqlite> {
                url: url.clone(),
                token: std::any::type_name::<Pool<Sqlite>>().to_string(),
                _db: PhantomData,
            })
            .export::<Pool<Sqlite>>();

        #[cfg(feature = "health")]
        {
            builder = builder
                .provider(crate::health::SqlxHealthIndicatorFactory::<Sqlite> {
                    url,
                    _db: PhantomData,
                })
                .export::<crate::health::SqlxHealthIndicator<Sqlite>>();
        }

        builder.global().build()
    }

    /// Register a second, named SQLite pool. See [`SqlxModule::postgres_named`].
    #[cfg(feature = "sqlite")]
    pub fn sqlite_named(name: impl Into<String>, url: impl Into<String>) -> DynamicModule {
        use sqlx::Sqlite;
        let name: String = name.into();
        DynamicModule::builder(format!("SqlxModule::sqlite::{name}"))
            .provider(SqlxPoolFactory::<Sqlite> {
                url: url.into(),
                token: name.clone(),
                _db: PhantomData,
            })
            .export_token(name)
            .global()
            .build()
    }
}
