//! A connect is one execution: the guards that admit a connection and the hook that greets it share
//! a context, and therefore a bag.
//!
//! The connect path used to build a context for the guards, store a client whose bag was a different
//! one, and build a third context for `on_connect` — so a guard's write reached neither the hook nor
//! anything after it.

use std::sync::{Mutex, OnceLock};

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;
use toni::async_trait;
use toni::context::{Extensions, HandlerContext, WsContext};
use toni::traits_helpers::Guard;
use toni::websocket::{WsClient, WsHandlerResult, WsMessage};
use toni::{
    injectable, module, new, on_connect, subscribe_message, subscriptions, use_guards,
    websocket_gateway,
};

use crate::common::TestServer;

#[derive(Clone, Debug, PartialEq)]
pub struct Principal(String);

/// What `on_connect` saw in the connecting client's bag.
static SEEN_AT_CONNECT: OnceLock<Mutex<Option<Principal>>> = OnceLock::new();

fn seen() -> &'static Mutex<Option<Principal>> {
    SEEN_AT_CONNECT.get_or_init(|| Mutex::new(None))
}

#[injectable]
pub struct AdmitAndStamp {}

#[async_trait]
impl Guard<WsContext> for AdmitAndStamp {
    async fn can_activate(&self, ctx: &WsContext) -> bool {
        // Only at connect. A gateway-level guard also runs per message, and stamping there too
        // would make the second assertion below unable to tell a leak from a fresh write.
        if ctx.event() == "connect" {
            ctx.extensions().insert(Principal("erin".into()));
        }
        true
    }
}

#[websocket_gateway("/ws-connect-execution")]
pub struct ConnectExecutionGateway {}

#[subscriptions]
#[use_guards(AdmitAndStamp)]
impl ConnectExecutionGateway {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    /// Reads the bag the connect guard wrote to, through the connect's own context.
    #[on_connect]
    async fn greet(&self, _client: &WsClient, ctx: &WsContext) -> Result<(), toni::WsError> {
        *seen().lock().unwrap() = ctx.extensions().get::<Principal>();
        Ok(())
    }

    /// Reports whether the connect execution's bag is visible here. It must not be: a message is a
    /// different execution, and what is meant to span the connection belongs in the session.
    #[subscribe_message("whoami")]
    async fn whoami(&self, ext: Extensions, _msg: WsMessage) -> WsHandlerResult {
        let seen = ext
            .get::<Principal>()
            .map(|p| p.0)
            .unwrap_or_else(|| "none".into());
        Ok(WsMessage::text(seen).into())
    }
}

#[module(providers: [AdmitAndStamp, ConnectExecutionGateway])]
impl ConnectExecutionModule {}

#[tokio_localset_test::localset_test]
async fn a_connect_guards_write_reaches_the_connect_hook() {
    *seen().lock().unwrap() = None;

    let server = TestServer::start(ConnectExecutionModule).await;
    let url = format!("ws://127.0.0.1:{}/ws-connect-execution", server.port);
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();

    ws.send(Message::Text(r#"{"event":"whoami"}"#.to_string().into()))
        .await
        .unwrap();
    let in_message = ws
        .next()
        .await
        .unwrap()
        .unwrap()
        .into_text()
        .unwrap()
        .to_string();

    assert_eq!(
        seen().lock().unwrap().clone(),
        Some(Principal("erin".into())),
        "the connect hook must see what the connect guard wrote — one execution, one bag"
    );
    assert_eq!(
        in_message, "none",
        "a message is a different execution: the connect bag must not carry into it"
    );
}
