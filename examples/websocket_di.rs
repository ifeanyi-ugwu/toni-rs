// WebSocket example with DI container integration and enhancers
//
// This example demonstrates:
// 1. Automatic gateway discovery
// 2. Global guards and interceptors applied to all gateways
// 3. Full integration of WebSocket with toni's DI system
// 4. Zero manual wiring - framework handles everything automatically

use toni::context::WsContext;
use toni::traits_helpers::{Guard, Interceptor, InterceptorNext};
use toni::websocket::{BroadcastModule, BroadcastService};
use toni::*;
use toni_macros::{injectable, module, websocket_gateway};

#[injectable]
pub struct WsAuthGuard;

#[async_trait]
impl Guard<WsContext> for WsAuthGuard {
    async fn can_activate(&self, ctx: &WsContext) -> bool {
        println!("[WsAuthGuard] Checking authentication...");
        if let Some(token) = ctx.client().handshake.headers.get("x-auth-token") {
            println!("[WsAuthGuard] ✅ Auth token found: {}", token);
            return true;
        }
        println!("[WsAuthGuard] ❌ No auth token - connection rejected");
        false
    }
}

#[injectable]
pub struct WsLoggingInterceptor;

#[async_trait]
impl Interceptor<WsContext> for WsLoggingInterceptor {
    async fn intercept(
        &self,
        ctx: &mut WsContext,
        next: Box<dyn InterceptorNext<WsContext>>,
    ) {
        println!("[WsLoggingInterceptor] 📥 Incoming message");
        println!("  Client: {}", ctx.client().id);
        println!("  Event: {}", ctx.event());
        next.run(ctx).await;
        println!("[WsLoggingInterceptor] 📤 Message processed");
    }
}

#[websocket_gateway("/chat", pub struct ChatGateway {
    broadcast: BroadcastService,
})]
#[use_guards(WsAuthGuard)]
#[use_interceptors(WsLoggingInterceptor)]
impl ChatGateway {
    pub fn new(broadcast: BroadcastService) -> Self {
        Self { broadcast }
    }

    #[subscribe_message("message")]
    async fn handle_message(
        &self,
        client: toni::WsClient,
        message: toni::WsMessage,
    ) -> toni::WsHandlerResult {
        let text = message
            .as_text()
            .ok_or_else(|| toni::WsError::InvalidMessage("Expected text message".into()))?;

        println!("[ChatGateway] Received from {}: {}", client.id, text);

        let response = format!("Broadcast: {}", text);
        self.broadcast
            .to_all()
            .send_event("message", &response)
            .await?;

        Ok(toni::WsHandlerOutput::Empty)
    }

    #[subscribe_message("ping")]
    async fn handle_ping(
        &self,
        _client: toni::WsClient,
        _message: toni::WsMessage,
    ) -> toni::WsHandlerResult {
        Ok(toni::WsMessage::text("pong").into())
    }
}

#[module(
    imports: [BroadcastModule::new()],
    providers: [
        ChatGateway,
        WsAuthGuard,
        WsLoggingInterceptor
    ]
)]
struct ChatModule;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("🚀 WebSocket DI Example - Automatic Gateway Discovery\n");
    println!("This example demonstrates:");
    println!("  • Zero manual wiring - framework auto-discovers gateways");
    println!("  • BroadcastService injected via DI");
    println!("  • Global guards and interceptors applied to all gateways\n");

    println!("WebSocket endpoint: ws://127.0.0.1:8080/chat\n");

    println!("Test WITHOUT auth token (will be rejected by guard):");
    println!(r#"  websocat ws://127.0.0.1:8080/chat"#);
    println!();

    println!("Test WITH auth token (will succeed):");
    println!(r#"  websocat -H='X-Auth-Token: secret123' ws://127.0.0.1:8080/chat"#);
    println!(r#"  Send: {{"event": "message", "data": "Hello"}}"#);
    println!();

    println!("Alternative header syntax:");
    println!(r#"  websocat --header='X-Auth-Token: secret123' ws://127.0.0.1:8080/chat"#);
    println!();

    let factory = ToniFactory::new();
    let mut app = factory.create_with(ChatModule).await;

    app.use_http_adapter(toni_axum::AxumAdapter::new(), 8080, "127.0.0.1")
        .unwrap();

    println!("✅ Server ready - guards and interceptors active!\n");

    app.start().await?;
    Ok(())
}
