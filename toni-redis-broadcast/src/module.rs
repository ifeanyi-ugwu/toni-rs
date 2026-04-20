use toni::{BroadcastService, DynamicModule};

use crate::{
    provider::{RedisBroadcastServiceFactory, SharedBroadcastServiceProviderFactory},
    service::RedisBroadcastService,
};

pub struct RedisBroadcastModule;

impl RedisBroadcastModule {
    /// Wire Redis-backed broadcasting into the application.
    ///
    /// Registers two providers globally:
    /// - `BroadcastService` — the in-process service the framework uses to wire
    ///   WebSocket connection callbacks. Shared with `RedisBroadcastService` so
    ///   both see the same connected clients.
    /// - `RedisBroadcastService` — the injectable you use in gateways and
    ///   services to broadcast across processes.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// #[module(imports: [RedisBroadcastModule::for_root("redis://127.0.0.1/")])]
    /// struct AppModule;
    /// ```
    pub fn for_root(url: impl Into<String>) -> DynamicModule {
        let url = url.into();
        // Created once here; cloned cheaply (Arc-backed) into both providers so
        // they share the same WsClientMap and ConnectionManager.
        let local_bs = BroadcastService::new();

        DynamicModule::builder("RedisBroadcastModule")
            .provider(SharedBroadcastServiceProviderFactory {
                instance: local_bs.clone(),
            })
            .provider(RedisBroadcastServiceFactory { url, local_bs })
            .export::<BroadcastService>()
            .export::<RedisBroadcastService>()
            .global()
            .build()
    }
}
