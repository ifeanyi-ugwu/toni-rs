//! `Validated<Payload<T>>` on the transports whose message is the payload.
//!
//! The wrapper is generic over the context, so the same declaration validates a
//! WebSocket frame and an RPC call. Each test proves both directions: a payload
//! that satisfies the `validator` attributes reaches the handler, and one that
//! does not is refused before the handler runs.

use std::time::Duration;

use serde::Deserialize;
use toni::context::RpcContext;
use toni::extractors::{Payload, Validated};
use toni::module;
use toni::rpc::{RpcData, RpcError};
use toni::websocket::{WsClient, WsHandlerResult, WsMessage};
use toni_macros::{controller, new, patterns, subscriptions, websocket_gateway};
use validator::Validate;

use crate::common::TestServer;

#[derive(Debug, Deserialize, Validate)]
pub struct CreateUser {
    #[validate(length(min = 3))]
    name: String,
}

// ---- WebSocket ---------------------------------------------------------------

#[websocket_gateway("/ws-validated")]
pub struct ValidatedGateway {}

#[subscriptions]
impl ValidatedGateway {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[subscribe_message("users.create")]
    async fn create(
        &self,
        _c: WsClient,
        Validated(Payload(dto)): Validated<Payload<CreateUser>>,
    ) -> WsHandlerResult {
        Ok(WsMessage::text(format!("created:{}", dto.name)).into())
    }
}

#[module(providers: [ValidatedGateway])]
impl ValidatedWsModule {}

// ---- RPC ---------------------------------------------------------------------

#[controller]
pub struct ValidatedRpcController {}

#[patterns]
impl ValidatedRpcController {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[message_pattern("users.create")]
    async fn create(
        &self,
        Validated(Payload(dto)): Validated<Payload<CreateUser>>,
    ) -> Result<RpcData, RpcError> {
        Ok(RpcData::json(serde_json::json!(format!(
            "created:{}",
            dto.name
        ))))
    }
}

#[module(controllers: [ValidatedRpcController])]
impl ValidatedRpcModule {}

// ---- TCP helpers -------------------------------------------------------------

async fn pick_free_port() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

async fn start_rpc_server(module: impl toni::ModuleMetadata + 'static) -> u16 {
    use toni::toni_factory::ToniFactory;
    let port = pick_free_port().await;
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = ToniFactory::create(module).await.unwrap();
        app.use_rpc_adapter(toni_tcp::TcpAdapter::new("127.0.0.1", port))
            .unwrap();
        app.start().await.unwrap();
    });
    tokio::task::spawn_local(async move { local.await });
    port
}

/// `TcpAdapter` binds inside its serve future, so there's no readiness signal
/// from the framework. Retry connect until success or the deadline expires.
async fn connect_with_retry(port: u16) -> tokio::net::TcpStream {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        match tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port)).await {
            Ok(s) => return s,
            Err(_) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(e) => panic!("RPC server never accepted on port {}: {}", port, e),
        }
    }
}

async fn tcp_rpc(port: u16, pattern: &str, data: serde_json::Value) -> serde_json::Value {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let stream = connect_with_retry(port).await;
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    let mut frame = serde_json::json!({"pattern": pattern, "data": data, "id": "1"}).to_string();
    frame.push('\n');
    writer.write_all(frame.as_bytes()).await.unwrap();

    let mut line = String::new();
    reader.read_line(&mut line).await.unwrap();
    serde_json::from_str(line.trim()).unwrap()
}

// ---- tests -------------------------------------------------------------------

/// An RPC payload is validated by the handler's own signature.
#[tokio_localset_test::localset_test]
async fn rpc_validated_payload_admits_valid_and_refuses_invalid() {
    let port = start_rpc_server(ValidatedRpcModule).await;

    let ok = tcp_rpc(port, "users.create", serde_json::json!({"name": "ada"})).await;
    assert_eq!(ok["response"], "created:ada");

    let rejected = tcp_rpc(port, "users.create", serde_json::json!({"name": "jo"})).await;
    assert_eq!(rejected["response"]["status"], "error");
    assert!(
        rejected["response"]["message"]
            .as_str()
            .unwrap()
            .contains("name"),
        "the refusal names the field that failed: {}",
        rejected["response"]
    );
}

/// The same wrapper over the same DTO, validating a WebSocket frame.
#[tokio_localset_test::localset_test]
async fn ws_validated_payload_admits_valid_and_refuses_invalid() {
    use futures_util::{SinkExt, StreamExt};

    let server = TestServer::start(ValidatedWsModule).await;
    let url = format!("ws://127.0.0.1:{}/ws-validated", server.port);
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();

    ws.send(tokio_tungstenite::tungstenite::Message::Text(
        r#"{"event":"users.create","name":"ada"}"#.to_string().into(),
    ))
    .await
    .unwrap();
    let reply = ws.next().await.unwrap().unwrap();
    assert_eq!(reply.to_text().unwrap(), "created:ada");

    ws.send(tokio_tungstenite::tungstenite::Message::Text(
        r#"{"event":"users.create","name":"jo"}"#.to_string().into(),
    ))
    .await
    .unwrap();
    let reply = ws.next().await.unwrap().unwrap();
    let json: serde_json::Value = serde_json::from_str(reply.to_text().unwrap()).unwrap();
    assert_eq!(json["status"], "error");
    assert!(
        json["message"].as_str().unwrap().contains("name"),
        "the refusal names the field that failed: {}",
        json
    );
}
