use redis::aio::ConnectionManager;
use toni::DynamicModule;

use crate::connection::RedisConnectionFactory;

pub struct RedisModule;

impl RedisModule {
    pub fn for_root(url: impl Into<String>) -> DynamicModule {
        let url: String = url.into();

        #[allow(unused_mut)]
        let mut builder = DynamicModule::builder("RedisModule")
            .provider(RedisConnectionFactory {
                url: url.clone(),
                token: std::any::type_name::<ConnectionManager>().to_string(),
            })
            .export::<ConnectionManager>();

        #[cfg(feature = "health")]
        {
            builder = builder
                .provider(crate::health::RedisHealthIndicatorFactory { url })
                .export::<crate::health::RedisHealthIndicator>();
        }

        builder.global().build()
    }

    /// Register a second, named Redis connection.
    ///
    /// `for_root` provides one `ConnectionManager` injectable by type. When an application needs
    /// more than one connection, each additional one is registered under a name and injected by
    /// that name — the type alone can no longer tell them apart.
    ///
    /// ```ignore
    /// #[module(imports: [
    ///     RedisModule::for_root(env!("PRIMARY_URL")),
    ///     RedisModule::for_root_named("cache", env!("CACHE_URL")),
    /// ])]
    /// pub struct AppModule;
    /// ```
    ///
    /// ```ignore
    /// #[injectable]
    /// pub struct SessionService {
    ///     #[inject("cache")]
    ///     redis: ConnectionManager,
    /// }
    /// ```
    ///
    /// The name is a global identifier: two connections cannot share one, and reusing a name
    /// across integrations is refused at startup. The connection only is registered — the health
    /// indicator is attached to the default `for_root` connection.
    pub fn for_root_named(name: impl Into<String>, url: impl Into<String>) -> DynamicModule {
        let name: String = name.into();
        DynamicModule::builder(format!("RedisModule::{name}"))
            .provider(RedisConnectionFactory {
                url: url.into(),
                token: name.clone(),
            })
            .export_token(name)
            .global()
            .build()
    }
}
