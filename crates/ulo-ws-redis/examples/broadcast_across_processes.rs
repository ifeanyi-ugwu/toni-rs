//! Broadcasting to WebSocket clients held by a different process.
//!
//! The in-memory `BroadcastModule` reaches only the clients this process is
//! holding, which is correct until there are two processes. `RedisBroadcastModule`
//! replaces it — import one or the other, never both — and publishes through
//! Redis so a message sent here arrives at a client connected there.
//!
//! Two consequences a caller sees. `send()` returns `Ok(0)` rather than a
//! delivery count: the recipients are in other processes and cannot be counted
//! at publish time. And room membership becomes async, because it is Redis
//! state rather than a local map.
//!
//! Run two of these on different ports and connect a client to each; a message
//! sent to the room arrives at both.
//!
//!     docker run -d -p 6379:6379 redis
//!     REDIS_URL=redis://127.0.0.1:6379 PORT=3000 \
//!         cargo run -p ulo-ws-redis --example broadcast_across_processes
//!     REDIS_URL=redis://127.0.0.1:6379 PORT=3001 \
//!         cargo run -p ulo-ws-redis --example broadcast_across_processes
//!
//!     websocat ws://127.0.0.1:3000/chat
//!     websocat ws://127.0.0.1:3001/chat
//!     {"event":"join","data":{"room":"lobby"}}
//!     {"event":"say","data":{"room":"lobby","text":"hello from wherever"}}

use serde::Deserialize;
use ulo::extract::Payload;
use ulo::prelude::*;
use ulo::ws::{WsClient, WsHandlerOutput, WsHandlerResult, WsMessage};
use ulo_http_hyper::AxumAdapter;
use ulo_macros::{module, new, subscriptions, websocket_gateway};
use ulo_ws_redis::{RedisBroadcastModule, RedisBroadcastService};

#[derive(Deserialize)]
pub struct Join {
    room: String,
}

#[derive(Deserialize)]
pub struct Say {
    room: String,
    text: String,
}

#[websocket_gateway("/chat")]
pub struct ChatGateway {
    #[inject]
    broadcast: RedisBroadcastService,
}

#[subscriptions]
impl ChatGateway {
    #[new]
    pub fn new(broadcast: RedisBroadcastService) -> Self {
        Self { broadcast }
    }

    #[subscribe_message("join")]
    async fn join(&self, client: WsClient, Payload(join): Payload<Join>) -> WsHandlerResult {
        // Async, unlike the in-memory service: membership lives in Redis.
        self.broadcast.join_room(&client.id, &join.room).await.ok();
        Ok(WsMessage::text(format!("joined {}", join.room)).into())
    }

    #[subscribe_message("say")]
    async fn say(&self, Payload(say): Payload<Say>) -> WsHandlerResult {
        // `Ok(0)` even when clients are listening elsewhere — the count of
        // recipients in other processes is not knowable at publish time.
        self.broadcast
            .to_room(&say.room)
            .send_event("said", say.text)
            .await
            .ok();
        Ok(Items::Empty)
    }
}

#[module(
    imports: [RedisBroadcastModule::for_root(
        std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".into())
    )],
    providers: [ChatGateway]
)]
impl AppModule {}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(3000);

    let mut app = UloFactory::create(AppModule).await?;
    app.use_http_adapter(AxumAdapter::new(), ("127.0.0.1", port))?;
    println!("chat on ws://127.0.0.1:{port}/chat");
    app.start().await?;
    Ok(())
}
