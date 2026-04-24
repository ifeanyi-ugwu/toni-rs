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
            .provider(PgPoolFactory { url: url.clone() })
            .export::<Pool<AsyncPgConnection>>();

        #[cfg(feature = "health")]
        {
            builder = builder
                .provider(crate::health::PgHealthIndicatorFactory { url })
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
            .provider(MySqlPoolFactory { url: url.clone() })
            .export::<Pool<AsyncMysqlConnection>>();

        #[cfg(feature = "health")]
        {
            builder = builder
                .provider(crate::health::MySqlHealthIndicatorFactory { url })
                .export::<crate::health::MySqlHealthIndicator>();
        }

        builder.global().build()
    }
}
