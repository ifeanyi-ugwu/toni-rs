//! A guard and an interceptor that each run on HTTP, RPC, and WebSocket.
//!
//! `Guard<C>` and `Interceptor<C>` take the per-request context type as a
//! parameter, so a struct can implement them three times — once per
//! transport — with reading code shaped for that transport's data source.
//! Each `impl` is the registration: the framework detects which transports a
//! provider serves from the contexts it implements, so the same struct runs on
//! all three. A guard implemented only for `Guard<HttpContext>` cannot be
//! attached to a gRPC method because the type system rejects the cast.
//!
//! Run with:  cargo run --example multi_protocol_context
//!
//! HTTP — `Authorization` header:
//!   curl -H 'Authorization: Bearer valid-secret' http://127.0.0.1:3000/api/orders
//!   curl http://127.0.0.1:3000/api/orders                  → 403
//!
//! RPC — `auth` field in the JSON payload (the TCP adapter doesn't surface
//! per-call metadata; reading from the payload works on any adapter):
//!   echo '{"pattern":"order.create","data":{"auth":"valid-secret","item":"book","qty":2},"id":"r1"}' | nc 127.0.0.1 4000
//!   echo '{"pattern":"order.create","data":{"item":"book","qty":2},"id":"r2"}' | nc 127.0.0.1 4000
//!     → second request is rejected by the guard.
//!
//! WebSocket — `token` query param on the handshake URL:
//!   websocat 'ws://127.0.0.1:3000/orders-ws?token=valid-secret'
//!     → send {"event":"echo","data":"hi"}; server replies with {"echo":"hi"}.
//!   websocat 'ws://127.0.0.1:3000/orders-ws'
//!     → handshake guard rejects, server closes the connection.

use serde_json::json;
use toni::async_trait;
use toni::context::{HttpContext, RpcContext, WsContext};
use toni::traits_helpers::{Guard, Interceptor, InterceptorNext};
use toni::websocket::{WsClient, WsError, WsHandlerResult, WsMessage};
use toni::*;
use toni_macros::{injectable, module, new, rpc_controller, subscriptions, websocket_gateway};

// ---- one guard, three transport-shaped impls --------------------------------

#[injectable]
pub struct UniversalAuthGuard {}
impl UniversalAuthGuard {}

#[async_trait]
impl Guard<HttpContext> for UniversalAuthGuard {
    async fn can_activate(&self, ctx: &HttpContext) -> bool {
        ctx.request()
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|h| h.strip_prefix("Bearer "))
            .map_or(false, |t| t == "valid-secret")
    }
}

#[async_trait]
impl Guard<RpcContext> for UniversalAuthGuard {
    async fn can_activate(&self, ctx: &RpcContext) -> bool {
        ctx.data()
            .as_json()
            .and_then(|v| v.get("auth").and_then(|a| a.as_str()))
            .map_or(false, |t| t == "valid-secret")
    }
}

#[async_trait]
impl Guard<WsContext> for UniversalAuthGuard {
    async fn can_activate(&self, ctx: &WsContext) -> bool {
        ctx.client()
            .handshake
            .query
            .get("token")
            .map_or(false, |t| t == "valid-secret")
    }
}

// ---- one logging interceptor, three transport-shaped impls ------------------

#[injectable]
pub struct LoggingInterceptor {}
impl LoggingInterceptor {}

#[async_trait]
impl Interceptor<HttpContext> for LoggingInterceptor {
    async fn intercept(&self, ctx: &mut HttpContext, next: Box<dyn InterceptorNext<HttpContext>>) {
        let req = ctx.request();
        println!(
            "[HTTP]      {} {} (agent: {:?})",
            req.method,
            req.uri,
            req.headers.get("user-agent").and_then(|v| v.to_str().ok())
        );
        next.run(ctx).await;
    }
}

#[async_trait]
impl Interceptor<RpcContext> for LoggingInterceptor {
    async fn intercept(&self, ctx: &mut RpcContext, next: Box<dyn InterceptorNext<RpcContext>>) {
        println!(
            "[RPC]       pattern='{}' data={:?}",
            ctx.pattern(),
            ctx.data()
        );
        next.run(ctx).await;
    }
}

#[async_trait]
impl Interceptor<WsContext> for LoggingInterceptor {
    async fn intercept(&self, ctx: &mut WsContext, next: Box<dyn InterceptorNext<WsContext>>) {
        println!(
            "[WebSocket] event='{}' client={} message={:?}",
            ctx.event(),
            ctx.client().id,
            ctx.message()
        );
        next.run(ctx).await;
    }
}

// ---- HTTP controller --------------------------------------------------------

#[controller("/api")]
pub struct OrdersHttp {}

#[routes]
#[use_guards(UniversalAuthGuard)]
#[use_interceptors(LoggingInterceptor)]
impl OrdersHttp {
    #[get("/orders")]
    fn list(&self) -> Body {
        Body::json(json!({ "orders": [{ "id": 1001, "item": "book" }] }))
    }
}

// ---- RPC controller ---------------------------------------------------------

#[rpc_controller(pub struct OrdersRpc {})]
#[use_guards(UniversalAuthGuard)]
#[use_interceptors(LoggingInterceptor)]
impl OrdersRpc {
    #[message_pattern("order.create")]
    async fn create(&self, data: RpcData, _ctx: &context::RpcContext) -> Result<RpcData, RpcError> {
        let payload = data
            .as_json()
            .ok_or_else(|| RpcError::Internal("expected JSON payload".into()))?;
        let item = payload["item"].as_str().unwrap_or("unknown");
        let qty = payload["qty"].as_u64().unwrap_or(1);
        Ok(RpcData::json(
            json!({ "id": 1001, "item": item, "qty": qty, "status": "created" }),
        ))
    }
}

// ---- WebSocket gateway ------------------------------------------------------

#[websocket_gateway("/orders-ws")]
pub struct OrdersWs {}
#[subscriptions]
#[use_guards(UniversalAuthGuard)]
#[use_interceptors(LoggingInterceptor)]
impl OrdersWs {
    #[subscribe_message("echo")]
    async fn echo(&self, _client: WsClient, msg: WsMessage) -> WsHandlerResult {
        let text = msg
            .as_text()
            .ok_or_else(|| WsError::InvalidMessage("expected text frame".into()))?;
        let payload: serde_json::Value =
            serde_json::from_str(text).unwrap_or(serde_json::Value::Null);
        let data = payload
            .get("data")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        Ok(WsMessage::text(json!({ "echo": data }).to_string()).into())
    }
}

// ---- module + bootstrap -----------------------------------------------------

#[module(
    providers: [
        UniversalAuthGuard,
        LoggingInterceptor,
        OrdersRpc,
        OrdersWs,
    ],
    controllers: [OrdersHttp],
)]
impl AppModule {}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("multi-protocol example");
    println!("  HTTP : http://127.0.0.1:3000/api/orders");
    println!("  RPC  : 127.0.0.1:4000  (newline-delimited JSON over TCP)");
    println!("  WS   : ws://127.0.0.1:3000/orders-ws");
    println!();
    println!("Token: send `valid-secret` as Bearer (HTTP), `authorization` metadata (RPC),");
    println!("or `?token=` query param (WS).");
    println!();

    let mut app = ToniFactory::new()
        .create_with(AppModule::module_definition())
        .await;

    app.use_http_adapter(toni_axum::AxumAdapter::new(), 3000, "127.0.0.1")
        .unwrap();
    app.use_rpc_adapter(toni_tcp::TcpAdapter::new("127.0.0.1", 4000))
        .unwrap();

    app.start().await?;
    Ok(())
}
