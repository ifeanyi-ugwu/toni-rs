//! A connection's session: what a connect guard establishes, every later execution on that
//! connection reads, and a reconnect does not.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;
use toni::async_trait;
use toni::context::{HandlerContext, WsContext};
use toni::traits_helpers::Guard;
use toni::websocket::{Session, WsClient, WsHandlerResult, WsMessage};
use toni::{
    injectable, module, new, on_connect, on_disconnect, subscribe_message, subscriptions,
    use_guards, websocket_gateway, DisconnectReason,
};

use crate::common::TestServer;

#[derive(Clone, Debug, PartialEq)]
pub struct Principal(String);

/// Numbered per connect, so two connections can be told apart by what their sessions hold.
static NEXT_CONNECTION: AtomicU64 = AtomicU64::new(1);

/// What `on_disconnect` found in the session it was closing.
static AT_TEARDOWN: OnceLock<Mutex<Option<Principal>>> = OnceLock::new();

fn teardown() -> &'static Mutex<Option<Principal>> {
    AT_TEARDOWN.get_or_init(|| Mutex::new(None))
}

#[injectable]
pub struct AuthenticateOnce {}

#[async_trait]
impl Guard<WsContext> for AuthenticateOnce {
    async fn can_activate(&self, ctx: &WsContext) -> bool {
        // Only at connect. The point of the session is that later executions need not repeat this.
        if ctx.event() == "connect" {
            let n = NEXT_CONNECTION.fetch_add(1, Ordering::SeqCst);
            ctx.session().insert(Principal(format!("erin-{n}")));
        }
        true
    }
}

#[websocket_gateway("/ws-session")]
pub struct SessionGateway {}

#[subscriptions]
#[use_guards(AuthenticateOnce)]
impl SessionGateway {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[on_connect]
    async fn greet(&self, _client: &WsClient) -> Result<(), toni::WsError> {
        Ok(())
    }

    /// Takes the session as a parameter, which is the whole of what a handler needs to do to read
    /// connection state.
    #[subscribe_message("whoami")]
    async fn whoami(&self, session: Session) -> WsHandlerResult {
        let who = session
            .get::<Principal>()
            .map(|p| p.0)
            .unwrap_or_else(|| "none".into());
        Ok(WsMessage::text(who).into())
    }

    /// The execution's own bag, for contrast: empty on every message.
    #[subscribe_message("execution")]
    async fn execution(&self, ctx: &WsContext) -> WsHandlerResult {
        let who = ctx
            .extensions()
            .get::<Principal>()
            .map(|p| p.0)
            .unwrap_or_else(|| "none".into());
        Ok(WsMessage::text(who).into())
    }

    #[on_disconnect]
    async fn record(&self, _client: &WsClient, ctx: &WsContext) {
        *teardown().lock().unwrap() = ctx.session().get::<Principal>();
    }
}

#[module(providers: [AuthenticateOnce, SessionGateway])]
impl SessionModule {}

async fn ask(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    event: &str,
) -> String {
    ws.send(Message::Text(format!(r#"{{"event":"{event}"}}"#).into()))
        .await
        .unwrap();
    ws.next()
        .await
        .unwrap()
        .unwrap()
        .into_text()
        .unwrap()
        .to_string()
}

#[tokio_localset_test::localset_test]
async fn a_connect_guards_write_is_read_by_every_later_message() {
    let server = TestServer::start(SessionModule).await;
    let url = format!("ws://127.0.0.1:{}/ws-session", server.port);
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();

    let first = ask(&mut ws, "whoami").await;
    assert!(
        first.starts_with("erin-"),
        "the connect guard's write is readable: {first}"
    );
    assert_eq!(
        ask(&mut ws, "whoami").await,
        first,
        "the session outlives one message, not just the first"
    );
    assert_eq!(
        ask(&mut ws, "execution").await,
        "none",
        "the execution's own bag is still per message"
    );
}

/// Each connection gets its own store. Sharing one across connections, or keying it anywhere but the
/// connection, would hand the second client the first one's principal.
#[tokio_localset_test::localset_test]
async fn each_connection_gets_its_own_session() {
    let server = TestServer::start(SessionModule).await;
    let url = format!("ws://127.0.0.1:{}/ws-session", server.port);

    let (mut first, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let one_before = ask(&mut first, "whoami").await;

    let (mut second, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let two = ask(&mut second, "whoami").await;

    // Reading the first connection again *after* the second has connected is what separates the two
    // stores from one shared store. Reading before would pass either way, the values differing only
    // because the guard numbers each connect.
    let one_after = ask(&mut first, "whoami").await;

    assert_eq!(
        one_after, one_before,
        "the second connection overwrote the first one's session: {one_before} became {one_after}"
    );
    assert_ne!(
        one_after, two,
        "two connections must not share a session: {one_after} / {two}"
    );
}

/// Teardown reads the session through the disconnect's own context — the last execution on the
/// connection, and the only chance to see what it held.
#[tokio_localset_test::localset_test]
async fn teardown_reads_the_session_it_is_closing() {
    *teardown().lock().unwrap() = None;

    let server = TestServer::start(SessionModule).await;
    let url = format!("ws://127.0.0.1:{}/ws-session", server.port);
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let who = ask(&mut ws, "whoami").await;

    ws.close(None).await.ok();
    drop(ws);
    for _ in 0..50 {
        if teardown().lock().unwrap().is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    assert_eq!(
        teardown().lock().unwrap().clone(),
        Some(Principal(who.clone())),
        "on_disconnect must see the principal the connect guard established"
    );
}
