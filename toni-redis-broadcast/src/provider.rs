use std::{any::Any, sync::Arc};

use async_trait::async_trait;
use futures_util::StreamExt;
use toni::{
    BroadcastService, FxHashMap,
    traits_helpers::{Injectable, Provider, ProviderContext, ProviderFactory},
};

use crate::{
    message::RedisBroadcastPayload,
    service::{RedisBroadcastService, deliver_locally},
};

// =============================================================================
// SharedBroadcastServiceProvider
// =============================================================================
// Registers the pre-built BroadcastService under its own DI token so that
// toni_application.rs can find it and wire ws_client_map into the WS callbacks.

pub(crate) struct SharedBroadcastServiceProviderFactory {
    pub instance: BroadcastService,
}

#[async_trait]
impl ProviderFactory for SharedBroadcastServiceProviderFactory {
    fn get_token(&self) -> String {
        toni::di::token_of::<BroadcastService>()
    }

    async fn build(&self, _deps: FxHashMap<String, Injectable>) -> Injectable {
        Injectable::new(
            Arc::new(Box::new(SharedBroadcastServiceProvider {
                instance: self.instance.clone(),
            })),
            vec![],
        )
    }
}

struct SharedBroadcastServiceProvider {
    instance: BroadcastService,
}

#[async_trait]
impl Provider for SharedBroadcastServiceProvider {
    fn get_token(&self) -> String {
        toni::di::token_of::<BroadcastService>()
    }

    async fn execute(
        &self,
        _params: Vec<Box<dyn Any + Send>>,
        _ctx: ProviderContext,
    ) -> Box<dyn Any + Send> {
        Box::new(self.instance.clone())
    }
}

// =============================================================================
// RedisBroadcastServiceFactory
// =============================================================================
// Connects to Redis (publisher connection + pubsub connection), spawns the
// subscriber background task, and registers RedisBroadcastService in DI.

pub(crate) struct RedisBroadcastServiceFactory {
    pub url: String,
    pub local_bs: BroadcastService,
}

/// Unique identifier for this process instance. Used as the private Pub/Sub
/// channel name so targeted `to_client` publishes reach only the process that
/// holds that client rather than fanning out to every process.
fn make_process_id() -> String {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{pid}-{nanos}")
}

#[async_trait]
impl ProviderFactory for RedisBroadcastServiceFactory {
    fn get_token(&self) -> String {
        toni::di::token_of::<RedisBroadcastService>()
    }

    async fn build(&self, _deps: FxHashMap<String, Injectable>) -> Injectable {
        let client = redis::Client::open(self.url.as_str()).unwrap_or_else(|e| {
            panic!(
                "toni-redis-broadcast: invalid Redis URL '{}': {e}",
                self.url
            )
        });

        let publisher: redis::aio::MultiplexedConnection = client
            .get_multiplexed_async_connection()
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "toni-redis-broadcast: failed to connect to '{}': {e}",
                    self.url
                )
            });

        let mut pubsub = client.get_async_pubsub().await.unwrap_or_else(|e| {
            panic!(
                "toni-redis-broadcast: failed to open pubsub connection to '{}': {e}",
                self.url
            )
        });

        let process_id = make_process_id();

        // Subscribe to the global channel (all-process broadcasts) and the
        // private channel (targeted to_client publishes for this process only).
        let private_channel = format!("toni:broadcast:{process_id}");
        pubsub
            .subscribe("toni:broadcast")
            .await
            .unwrap_or_else(|e| {
                panic!("toni-redis-broadcast: failed to subscribe to broadcast channel: {e}")
            });
        pubsub
            .subscribe(&private_channel)
            .await
            .unwrap_or_else(|e| {
                panic!("toni-redis-broadcast: failed to subscribe to private channel '{private_channel}': {e}")
            });

        let local = self.local_bs.clone();
        let join_handle = tokio::spawn(async move {
            let mut stream = pubsub.into_on_message();
            while let Some(msg) = stream.next().await {
                let Ok(json) = msg.get_payload::<String>() else {
                    continue;
                };
                let Ok(payload) = serde_json::from_str::<RedisBroadcastPayload>(&json) else {
                    tracing::warn!("toni-redis-broadcast: failed to deserialize broadcast payload");
                    continue;
                };
                deliver_locally(&local, payload).await;
            }
            tracing::debug!("toni-redis-broadcast: subscriber stream ended");
        });

        let service = RedisBroadcastService::new(
            self.local_bs.clone(),
            publisher,
            process_id,
            join_handle.abort_handle(),
        );

        Injectable::new(
            Arc::new(Box::new(RedisBroadcastServiceProvider {
                instance: service,
            })),
            vec![],
        )
    }
}

struct RedisBroadcastServiceProvider {
    instance: RedisBroadcastService,
}

#[async_trait]
impl Provider for RedisBroadcastServiceProvider {
    fn get_token(&self) -> String {
        toni::di::token_of::<RedisBroadcastService>()
    }

    async fn execute(
        &self,
        _params: Vec<Box<dyn Any + Send>>,
        _ctx: ProviderContext,
    ) -> Box<dyn Any + Send> {
        Box::new(self.instance.clone())
    }
}
