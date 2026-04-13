#[cfg(any(feature = "postgres", feature = "mysql"))]
use toni::DynamicModule;

pub struct DieselModule;

impl DieselModule {
    #[cfg(feature = "postgres")]
    pub fn postgres(url: impl Into<String>) -> DynamicModule {
        use diesel_async::{AsyncPgConnection, pooled_connection::deadpool::Pool};

        use crate::pool::PgPoolFactory;
        DynamicModule::builder("DieselModule::postgres")
            .provider(PgPoolFactory { url: url.into() })
            .export::<Pool<AsyncPgConnection>>()
            .global()
            .build()
    }

    #[cfg(feature = "mysql")]
    pub fn mysql(url: impl Into<String>) -> DynamicModule {
        use diesel_async::{AsyncMysqlConnection, pooled_connection::deadpool::Pool};

        use crate::pool::MySqlPoolFactory;
        DynamicModule::builder("DieselModule::mysql")
            .provider(MySqlPoolFactory { url: url.into() })
            .export::<Pool<AsyncMysqlConnection>>()
            .global()
            .build()
    }
}
