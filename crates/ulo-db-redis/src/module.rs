use crate::connection::RedisConnectionFactory;
use redis::aio::ConnectionManager;
use ulo::StartupCheck;
use ulo::di::{CheckedModule, DynamicModule, Key, token_of};

pub struct RedisModule;

impl RedisModule {
    pub fn for_root(url: impl Into<String>) -> CheckedModule {
        let url: String = url.into();

        CheckedModule::new(move |check: Option<StartupCheck>| {
            #[allow(unused_mut)]
            let mut builder = DynamicModule::builder("RedisModule")
                .provider(RedisConnectionFactory {
                    url: url.clone(),
                    token: ulo::di::token_of::<ConnectionManager>(),
                    check,
                })
                .export::<ConnectionManager>();

            #[cfg(feature = "health")]
            {
                builder = builder
                    .provider(crate::health::RedisHealthIndicatorFactory)
                    .export::<crate::health::RedisHealthIndicator>();
            }

            builder.global().build()
        })
    }

    /// Register a second Redis connection, under the marker `K`.
    ///
    /// `for_root` provides one `ConnectionManager` injectable by type. A second connection needs a
    /// slot of its own, named by a marker type whose slot holds a `ConnectionManager`:
    ///
    /// ```ignore
    /// key!(pub Cache: ConnectionManager);
    ///
    /// #[module(imports: [
    ///     RedisModule::for_root(env!("PRIMARY_URL")),
    ///     RedisModule::for_root_keyed::<Cache>(env!("CACHE_URL")),
    /// ])]
    /// pub struct AppModule;
    ///
    /// #[injectable]
    /// pub struct SessionService {
    ///     #[inject(Cache)]
    ///     redis: ConnectionManager,
    /// }
    /// ```
    ///
    /// Two connections under one marker are refused at startup, whichever integrations register
    /// them. The connection only is registered — the health indicator is attached to the default
    /// `for_root` connection.
    pub fn for_root_keyed<K>(url: impl Into<String>) -> CheckedModule
    where
        K: Key<Value = ConnectionManager>,
    {
        let url: String = url.into();

        CheckedModule::new(move |check: Option<StartupCheck>| {
            DynamicModule::builder(token_of::<K>())
                .provider(RedisConnectionFactory {
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
