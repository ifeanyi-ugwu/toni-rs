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
        DynamicModule::builder("SqlxModule::postgres")
            .provider(SqlxPoolFactory::<Postgres> {
                url: url.into(),
                _db: PhantomData,
            })
            .export::<Pool<Postgres>>()
            .global()
            .build()
    }

    #[cfg(feature = "mysql")]
    pub fn mysql(url: impl Into<String>) -> DynamicModule {
        use sqlx::{MySql, Pool};
        DynamicModule::builder("SqlxModule::mysql")
            .provider(SqlxPoolFactory::<MySql> {
                url: url.into(),
                _db: PhantomData,
            })
            .export::<Pool<MySql>>()
            .global()
            .build()
    }

    #[cfg(feature = "sqlite")]
    pub fn sqlite(url: impl Into<String>) -> DynamicModule {
        use sqlx::{Pool, Sqlite};
        DynamicModule::builder("SqlxModule::sqlite")
            .provider(SqlxPoolFactory::<Sqlite> {
                url: url.into(),
                _db: PhantomData,
            })
            .export::<Pool<Sqlite>>()
            .global()
            .build()
    }
}
