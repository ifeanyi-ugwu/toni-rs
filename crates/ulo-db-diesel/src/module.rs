#[cfg(any(feature = "postgres", feature = "mysql"))]
use ulo::StartupCheck;
use ulo::di::{CheckedModule, DynamicModule, Key, token_of};
pub struct DieselModule;

impl DieselModule {
    #[cfg(feature = "postgres")]
    pub fn postgres(url: impl Into<String>) -> CheckedModule {
        use diesel_async::{AsyncPgConnection, pooled_connection::deadpool::Pool};

        use crate::pool::PgPoolFactory;
        let url: String = url.into();

        CheckedModule::new(move |check: Option<StartupCheck>| {
            #[allow(unused_mut)]
            let mut builder = DynamicModule::builder("DieselModule::postgres")
                .provider(PgPoolFactory {
                    url: url.clone(),
                    token: ulo::di::token_of::<Pool<AsyncPgConnection>>(),
                    check,
                })
                .export::<Pool<AsyncPgConnection>>();

            #[cfg(feature = "health")]
            {
                builder = builder
                    .provider(crate::health::PgHealthIndicatorFactory)
                    .export::<crate::health::PgHealthIndicator>();
            }

            builder.global().build()
        })
    }

    #[cfg(feature = "mysql")]
    pub fn mysql(url: impl Into<String>) -> CheckedModule {
        use diesel_async::{AsyncMysqlConnection, pooled_connection::deadpool::Pool};

        use crate::pool::MySqlPoolFactory;
        let url: String = url.into();

        CheckedModule::new(move |check: Option<StartupCheck>| {
            #[allow(unused_mut)]
            let mut builder = DynamicModule::builder("DieselModule::mysql")
                .provider(MySqlPoolFactory {
                    url: url.clone(),
                    token: ulo::di::token_of::<Pool<AsyncMysqlConnection>>(),
                    check,
                })
                .export::<Pool<AsyncMysqlConnection>>();

            #[cfg(feature = "health")]
            {
                builder = builder
                    .provider(crate::health::MySqlHealthIndicatorFactory)
                    .export::<crate::health::MySqlHealthIndicator>();
            }

            builder.global().build()
        })
    }

    /// Register a second Postgres pool, under the marker `K`.
    ///
    /// `postgres` provides one `Pool<AsyncPgConnection>` injectable by type. A second one needs a
    /// slot of its own, named by a marker type whose slot holds a `Pool<AsyncPgConnection>`:
    ///
    /// ```ignore
    /// key!(pub Analytics: Pool<AsyncPgConnection>);
    ///
    /// #[module(imports: [
    ///     DieselModule::postgres(env!("PRIMARY_URL")),
    ///     DieselModule::postgres_keyed::<Analytics>(env!("ANALYTICS_URL")),
    /// ])]
    /// pub struct AppModule;
    ///
    /// #[injectable]
    /// pub struct ReportService {
    ///     #[inject(Analytics)]
    ///     pool: Pool<AsyncPgConnection>,
    /// }
    /// ```
    ///
    /// Two pools under one marker are refused at startup, whichever integrations register them. The
    /// pool only is registered — the health indicator is attached to the default `postgres` pool.
    #[cfg(feature = "postgres")]
    pub fn postgres_keyed<K>(url: impl Into<String>) -> CheckedModule
    where
        K: Key<Value = crate::PgPool>,
    {
        use crate::pool::PgPoolFactory;
        let url: String = url.into();
        CheckedModule::new(move |check: Option<StartupCheck>| {
            DynamicModule::builder(token_of::<K>())
                .provider(PgPoolFactory {
                    url: url.clone(),
                    token: token_of::<K>(),
                    check,
                })
                .export::<K>()
                .global()
                .build()
        })
    }

    /// Register a second MySQL pool, under the marker `K`. See [`DieselModule::postgres_keyed`].
    #[cfg(feature = "mysql")]
    pub fn mysql_keyed<K>(url: impl Into<String>) -> CheckedModule
    where
        K: Key<Value = crate::MySqlPool>,
    {
        use crate::pool::MySqlPoolFactory;
        let url: String = url.into();
        CheckedModule::new(move |check: Option<StartupCheck>| {
            DynamicModule::builder(token_of::<K>())
                .provider(MySqlPoolFactory {
                    url: url.clone(),
                    token: token_of::<K>(),
                    check,
                })
                .export::<K>()
                .global()
                .build()
        })
    }
}
