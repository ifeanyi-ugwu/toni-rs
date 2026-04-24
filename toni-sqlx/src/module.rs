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

    #[cfg(feature = "mysql")]
    pub fn mysql(url: impl Into<String>) -> DynamicModule {
        use sqlx::{MySql, Pool};
        let url: String = url.into();

        #[allow(unused_mut)]
        let mut builder = DynamicModule::builder("SqlxModule::mysql")
            .provider(SqlxPoolFactory::<MySql> {
                url: url.clone(),
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

    #[cfg(feature = "sqlite")]
    pub fn sqlite(url: impl Into<String>) -> DynamicModule {
        use sqlx::{Pool, Sqlite};
        let url: String = url.into();

        #[allow(unused_mut)]
        let mut builder = DynamicModule::builder("SqlxModule::sqlite")
            .provider(SqlxPoolFactory::<Sqlite> {
                url: url.clone(),
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
}
