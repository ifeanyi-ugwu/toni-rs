use redis::aio::ConnectionManager;
use toni::DynamicModule;

use crate::connection::RedisConnectionFactory;

pub struct RedisModule;

impl RedisModule {
    pub fn for_root(url: impl Into<String>) -> DynamicModule {
        DynamicModule::builder("RedisModule")
            .provider(RedisConnectionFactory { url: url.into() })
            .export::<ConnectionManager>()
            .global()
            .build()
    }
}
