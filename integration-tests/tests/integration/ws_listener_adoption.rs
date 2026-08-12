//! Conformance suite for separate-port WebSocket gateways serving on a socket
//! the caller bound, supplied through `use_websocket_listener`.
//!
//! The gateway declares `port = 19100` and nothing ever binds that number: the
//! socket handed over listens on an OS-assigned port instead. That separation
//! is the point of the test. The declared port selects which gateway the
//! socket belongs to, and where the socket listens is the socket's business,
//! so the application must report and serve on the address it was given.
//!
//! Because the declared port is a key rather than a reservation, every case
//! below can share one gateway and still run concurrently.

use std::net::SocketAddr;

use futures_util::{SinkExt, StreamExt};
use toni::module;
use toni::toni_factory::ToniFactory;
use toni::websocket::{WsClient, WsHandlerResult, WsMessage};
use toni_macros::{new, subscribe_message, subscriptions, websocket_gateway};

/// Never bound by anything. Its only job is to pair the gateway with the
/// socket passed to `use_websocket_listener`.
const DECLARED_PORT: u16 = 19100;

#[websocket_gateway("/adopted", port = 19100)]
pub struct AdoptedGateway {}

#[subscriptions]
impl AdoptedGateway {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[subscribe_message("ping")]
    async fn on_ping(&self, _client: WsClient, _msg: WsMessage) -> WsHandlerResult {
        Ok(WsMessage::text("pong").into())
    }
}

#[module(providers: [AdoptedGateway])]
struct AdoptedModule;

async fn case_serves_on_caller_socket(adapter: impl toni::WebSocketAdapter) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let expected = listener.local_addr().unwrap();

    let (addr_tx, addr_rx) = tokio::sync::oneshot::channel::<SocketAddr>();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = ToniFactory::create(AdoptedModule).await;
        app.use_websocket_adapter(adapter).unwrap();
        app.use_websocket_listener(DECLARED_PORT, listener).unwrap();
        let bound = app.bind().await.unwrap();
        let _ = addr_tx.send(
            bound
                .websocket
                .into_iter()
                .next()
                .expect("separate-port gateway must report an address"),
        );
        app.run().await;
    });
    tokio::task::spawn_local(async move { local.await });

    let reported = addr_rx.await.expect("WebSocket server failed to start");
    assert_eq!(
        reported, expected,
        "adapter reported a different address than the listener it was given"
    );
    assert_ne!(
        reported.port(),
        DECLARED_PORT,
        "the declared port was bound, so this proves nothing about adoption"
    );

    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{reported}/adopted"))
        .await
        .expect("adopted socket must accept a WebSocket handshake");

    ws.send(tokio_tungstenite::tungstenite::Message::Text(
        r#"{"event": "ping"}"#.to_string().into(),
    ))
    .await
    .unwrap();

    let reply = ws.next().await.unwrap().unwrap();
    assert_eq!(reply.to_text().unwrap(), "pong");
}

macro_rules! ws_adoption_suite {
    ($adapter_mod:ident, $adapter:expr) => {
        mod $adapter_mod {
            #[tokio_localset_test::localset_test]
            async fn serves_on_caller_supplied_listener() {
                super::case_serves_on_caller_socket($adapter).await;
            }
        }
    };
}

ws_adoption_suite!(axum, toni_axum::AxumAdapter::new());
ws_adoption_suite!(poem, toni_poem::PoemAdapter::new());
ws_adoption_suite!(salvo, toni_salvo::SalvoAdapter::new());
ws_adoption_suite!(tungstenite, toni_tungstenite::TungsteniteAdapter::new());
