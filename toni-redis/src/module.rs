use redis::aio::ConnectionManager;
use toni::DynamicModule;

use crate::connection::RedisConnectionFactory;

pub struct RedisModule;

impl RedisModule {
    pub fn for_root(url: impl Into<String>) -> DynamicModule {
        let url: String = url.into();

        #[allow(unused_mut)]
        let mut builder = DynamicModule::builder("RedisModule")
            .provider(RedisConnectionFactory { url: url.clone() })
            .export::<ConnectionManager>();

        #[cfg(feature = "health")]
        {
            builder = builder
                .provider(crate::health::RedisHealthIndicatorFactory { url })
                .export::<crate::health::RedisHealthIndicator>();
        }

        builder.global().build()
    }
}
