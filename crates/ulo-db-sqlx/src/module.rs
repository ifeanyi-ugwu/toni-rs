#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
use std::marker::PhantomData;

#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
use crate::pool::SqlxPoolFactory;
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
use ulo::StartupCheck;
use ulo::di::{CheckedModule, DynamicModule, Key, token_of};

pub struct SqlxModule;

impl SqlxModule {
    #[cfg(feature = "postgres")]
    pub fn postgres(url: impl Into<String>) -> CheckedModule {
        use sqlx::{Pool, Postgres};
        let url: String = url.into();

        CheckedModule::new(move |check: Option<StartupCheck>| {
            #[allow(unused_mut)]
            let mut builder = DynamicModule::builder("SqlxModule::postgres")
                .provider(SqlxPoolFactory::<Postgres> {
                    url: url.clone(),
                    token: ulo::di::token_of::<Pool<Postgres>>(),
                    check,
                    _db: PhantomData,
                })
                .export::<Pool<Postgres>>();

            #[cfg(feature = "health")]
            {
                builder = builder
                    .provider(crate::health::SqlxHealthIndicatorFactory::<Postgres> {
                        _db: PhantomData,
                    })
                    .export::<crate::health::SqlxHealthIndicator<Postgres>>();
            }

            builder.global().build()
        })
    }

    /// Register a second Postgres pool, under the marker `K`.
    ///
    /// `postgres` provides one `Pool<Postgres>` injectable by type. A second one needs a slot of
    /// its own, named by a marker type whose slot holds a `Pool<Postgres>`:
    ///
    /// ```ignore
    /// key!(pub Analytics: Pool<Postgres>);
    ///
    /// #[module(imports: [
    ///     SqlxModule::postgres(env!("PRIMARY_URL")),
    ///     SqlxModule::postgres_keyed::<Analytics>(env!("ANALYTICS_URL")),
    /// ])]
    /// pub struct AppModule;
    ///
    /// #[injectable]
    /// pub struct ReportService {
    ///     #[inject(Analytics)]
    ///     pool: Pool<Postgres>,
    /// }
    /// ```
    ///
    /// Two pools under one marker are refused at startup, whichever integrations register them. The
    /// pool only is registered — the health indicator is attached to the default `postgres` pool.
    #[cfg(feature = "postgres")]
    pub fn postgres_keyed<K>(url: impl Into<String>) -> CheckedModule
    where
        K: Key<Value = sqlx::Pool<sqlx::Postgres>>,
    {
        use sqlx::Postgres;
        let url: String = url.into();
        CheckedModule::new(move |check: Option<StartupCheck>| {
            DynamicModule::builder(token_of::<K>())
                .provider(SqlxPoolFactory::<Postgres> {
                    url: url.clone(),
                    token: token_of::<K>(),
                    check,
                    _db: PhantomData,
                })
                .export::<K>()
                .global()
                .build()
        })
    }

    #[cfg(feature = "mysql")]
    pub fn mysql(url: impl Into<String>) -> CheckedModule {
        use sqlx::{MySql, Pool};
        let url: String = url.into();

        CheckedModule::new(move |check: Option<StartupCheck>| {
            #[allow(unused_mut)]
            let mut builder = DynamicModule::builder("SqlxModule::mysql")
                .provider(SqlxPoolFactory::<MySql> {
                    url: url.clone(),
                    token: ulo::di::token_of::<Pool<MySql>>(),
                    check,
                    _db: PhantomData,
                })
                .export::<Pool<MySql>>();

            #[cfg(feature = "health")]
            {
                builder = builder
                    .provider(crate::health::SqlxHealthIndicatorFactory::<MySql> {
                        _db: PhantomData,
                    })
                    .export::<crate::health::SqlxHealthIndicator<MySql>>();
            }

            builder.global().build()
        })
    }

    /// Register a second MySQL pool, under the marker `K`. See [`SqlxModule::postgres_keyed`].
    #[cfg(feature = "mysql")]
    pub fn mysql_keyed<K>(url: impl Into<String>) -> CheckedModule
    where
        K: Key<Value = sqlx::Pool<sqlx::MySql>>,
    {
        use sqlx::MySql;
        let url: String = url.into();
        CheckedModule::new(move |check: Option<StartupCheck>| {
            DynamicModule::builder(token_of::<K>())
                .provider(SqlxPoolFactory::<MySql> {
                    url: url.clone(),
                    token: token_of::<K>(),
                    check,
                    _db: PhantomData,
                })
                .export::<K>()
                .global()
                .build()
        })
    }

    #[cfg(feature = "sqlite")]
    pub fn sqlite(url: impl Into<String>) -> CheckedModule {
        use sqlx::{Pool, Sqlite};
        let url: String = url.into();

        CheckedModule::new(move |check: Option<StartupCheck>| {
            #[allow(unused_mut)]
            let mut builder = DynamicModule::builder("SqlxModule::sqlite")
                .provider(SqlxPoolFactory::<Sqlite> {
                    url: url.clone(),
                    token: ulo::di::token_of::<Pool<Sqlite>>(),
                    check,
                    _db: PhantomData,
                })
                .export::<Pool<Sqlite>>();

            #[cfg(feature = "health")]
            {
                builder = builder
                    .provider(crate::health::SqlxHealthIndicatorFactory::<Sqlite> {
                        _db: PhantomData,
                    })
                    .export::<crate::health::SqlxHealthIndicator<Sqlite>>();
            }

            builder.global().build()
        })
    }

    /// Register a second SQLite pool, under the marker `K`. See [`SqlxModule::postgres_keyed`].
    #[cfg(feature = "sqlite")]
    pub fn sqlite_keyed<K>(url: impl Into<String>) -> CheckedModule
    where
        K: Key<Value = sqlx::Pool<sqlx::Sqlite>>,
    {
        use sqlx::Sqlite;
        let url: String = url.into();
        CheckedModule::new(move |check: Option<StartupCheck>| {
            DynamicModule::builder(token_of::<K>())
                .provider(SqlxPoolFactory::<Sqlite> {
                    url: url.clone(),
                    token: token_of::<K>(),
                    check,
                    _db: PhantomData,
                })
                .export::<K>()
                .global()
                .build()
        })
    }
}
