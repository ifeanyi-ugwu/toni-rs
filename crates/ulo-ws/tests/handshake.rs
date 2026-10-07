//! What a gateway decides before its first message: the subprotocol the 101 echoes, a refusal
//! before the upgrade under `refuse = handshake`, connect guards written by value and by closure,
//! and the session `session = T` creates before those guards run.

mod support;

use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::json;
use ulo::{BoxError, Dep, Guard, Module, ModuleDef, ModuleIdentity, injectable, routes};
use ulo_transport::ErrorKind;
use ulo_ws::{ConnectCx, ConnectRefused, OnConnect, Session, UpgradeHead, WsConnect};

use support::{Running, close_frame, hang_up, next_json, send_json, upgrade};

/// The close code graphql-transport-ws's reference server refuses a connection with when the
/// client offered none of its subprotocols.
const SUBPROTOCOL_NOT_ACCEPTABLE: u16 = 4406;

/// Speaks two versions of a protocol and refuses a connection that agreed on neither.
#[injectable]
struct Versioned;

#[routes]
#[ulo_ws::gateway(path = "/versioned", port = own, subprotocols = ["v2.chat", "v1.chat"])]
impl Versioned {
    #[ulo_ws::message("which")]
    fn which(&self, head: Dep<UpgradeHead>) -> Option<String> {
        head.subprotocol().map(str::to_owned)
    }
}

impl OnConnect for Versioned {
    async fn on_connect(&self, cx: &ConnectCx) -> Result<(), ConnectRefused> {
        if cx.head().subprotocol().is_some() {
            return Ok(());
        }
        Err(ConnectRefused::code(SUBPROTOCOL_NOT_ACCEPTABLE, "no subprotocol agreed")
            .unwrap_or_else(|error| panic!("4406 is a close code a frame carries: {error}")))
    }
}

/// Lists a subprotocol and proceeds without one.
#[injectable]
struct Lenient;

#[routes]
#[ulo_ws::gateway(path = "/lenient", port = own, subprotocols = ["v1.chat"])]
impl Lenient {
    #[ulo_ws::message("which")]
    fn which(&self, head: Dep<UpgradeHead>) -> Option<String> {
        head.subprotocol().map(str::to_owned)
    }
}

/// Lists no subprotocol.
#[injectable]
struct Unversioned;

#[routes]
#[ulo_ws::gateway(path = "/unversioned", port = own)]
impl Unversioned {
    #[ulo_ws::message("which")]
    fn which(&self, head: Dep<UpgradeHead>) -> Option<String> {
        head.subprotocol().map(str::to_owned)
    }
}

/// Admits an upgrade whose header `name` equals `value`.
struct HeaderIs {
    name: &'static str,
    value: &'static str,
}

impl Guard<WsConnect> for HeaderIs {
    async fn can_activate(&self, cx: &ConnectCx) -> Result<bool, BoxError> {
        Ok(cx.head().headers().get(self.name).is_some_and(|value| value == self.value))
    }
}

/// The tokens the closure-built guard admits, bound in the module.
struct Allowlist(Vec<&'static str>);

/// Admits an upgrade whose `x-token` is on the allowlist.
struct Allowed(Dep<Allowlist>);

impl Guard<WsConnect> for Allowed {
    async fn can_activate(&self, cx: &ConnectCx) -> Result<bool, BoxError> {
        let token = cx.head().headers().get("x-token").and_then(|token| token.to_str().ok());
        Ok(token.is_some_and(|token| self.0.0.contains(&token)))
    }
}

/// Built per connection from the upgrade request, an execution input: admits an upgrade from the
/// `/agent` user agent.
struct FromAgent(bool);

impl Guard<WsConnect> for FromAgent {
    async fn can_activate(&self, _cx: &ConnectCx) -> Result<bool, BoxError> {
        Ok(self.0)
    }
}

/// Refuses before the upgrade: no token is 403, a token without a user is 401.
#[injectable]
struct Strict;

#[routes]
#[ulo_ws::gateway(
    path = "/strict",
    port = own,
    refuse = handshake,
    max_connections = 1,
    connect_guards(value = HeaderIs { name: "x-token", value: "open" })
)]
impl Strict {
    #[ulo_ws::message("hello")]
    fn hello(&self) -> &'static str {
        "hello"
    }
}

impl OnConnect for Strict {
    async fn on_connect(&self, cx: &ConnectCx) -> Result<(), ConnectRefused> {
        match cx.head().headers().get("x-user") {
            Some(_) => Ok(()),
            None => Err(ConnectRefused::kind(ErrorKind::Unauthorized, "who are you?")),
        }
    }
}

/// Three connect guards, one per spelling: by value, by closure over the container, and by closure
/// built per execution from the upgrade request.
#[injectable]
struct Gated;

#[routes]
#[ulo_ws::gateway(
    path = "/gated",
    port = own,
    connect_guards(
        value = HeaderIs { name: "x-value", value: "yes" },
        with = |allowlist: Dep<Allowlist>| Allowed(allowlist),
        with(execution) = |head: Dep<UpgradeHead>| FromAgent(head.headers().get("user-agent").is_some_and(|agent| agent == "agent")),
    )
)]
impl Gated {
    #[ulo_ws::message("hello")]
    fn hello(&self) -> &'static str {
        "hello"
    }
}

/// A connection's count, from `Default`.
#[derive(Default)]
struct Tally(AtomicUsize);

/// Admits every connection, raising its session's count to 100 on the way: the session exists
/// before the connect guards run, and the handlers read the same one.
struct Seeds(Session<Tally>);

impl Guard<WsConnect> for Seeds {
    async fn can_activate(&self, _cx: &ConnectCx) -> Result<bool, BoxError> {
        self.0.0.fetch_add(100, Ordering::SeqCst);
        Ok(true)
    }
}

#[injectable]
struct Counted;

#[routes]
#[ulo_ws::gateway(path = "/counted", port = own, session = Tally, connect_guards(with = |tally: Session<Tally>| Seeds(tally)))]
impl Counted {
    #[ulo_ws::message("bump")]
    fn bump(&self, tally: Session<Tally>) -> usize {
        tally.0.fetch_add(1, Ordering::SeqCst) + 1
    }
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(Allowlist(vec!["t1", "t2"]));
        m.controller::<Versioned>();
        m.controller::<Lenient>();
        m.controller::<Unversioned>();
        m.controller::<Strict>();
        m.controller::<Gated>();
        m.controller::<Counted>();
    }
}

async fn start() -> Running {
    Running::start(Root, ulo_ws::Server::new("127.0.0.1:0")).await
}

/// `which` asked on `socket`: the subprotocol the connection's `UpgradeHead` carries.
async fn which(socket: &mut support::Socket) -> serde_json::Value {
    send_json(socket, &json!({ "event": "which", "id": 1 })).await;
    next_json(socket).await
}

#[tokio::test]
async fn the_101_echoes_the_first_listed_subprotocol_the_client_offered() {
    let app = start().await;
    for (offered, chosen) in [("v1.chat, v2.chat", "v2.chat"), ("v1.chat", "v1.chat"), ("other, v1.chat", "v1.chat")] {
        let mut answer = upgrade(app.addr, "/versioned", &[("Sec-WebSocket-Protocol", offered)]).await;
        assert_eq!(answer.status, 101, "offered {offered:?}: {}", answer.body);
        assert_eq!(answer.header("sec-websocket-protocol"), Some(chosen), "offered {offered:?}");
        let mut socket = answer.socket.take().unwrap_or_else(|| panic!("a 101 carries a socket"));
        assert_eq!(which(&mut socket).await, json!({ "id": 1, "data": chosen }));
        hang_up(socket).await;
    }
    app.stop().await;
}

#[tokio::test]
async fn offers_spread_over_two_header_lines_are_read_as_one_list() {
    let app = start().await;
    let answer = upgrade(app.addr, "/versioned", &[("Sec-WebSocket-Protocol", "other"), ("Sec-WebSocket-Protocol", "v1.chat")]).await;
    assert_eq!(answer.header("sec-websocket-protocol"), Some("v1.chat"));
    if let Some(socket) = answer.socket {
        hang_up(socket).await;
    }
    app.stop().await;
}

#[tokio::test]
async fn no_offered_subprotocol_gets_a_101_without_one_and_the_gateway_refuses() {
    let app = start().await;
    for headers in [&[("Sec-WebSocket-Protocol", "graphql-ws")][..], &[][..]] {
        let mut answer = upgrade(app.addr, "/versioned", headers).await;
        assert_eq!(answer.status, 101, "offered {headers:?}: {}", answer.body);
        assert_eq!(answer.header("sec-websocket-protocol"), None, "offered {headers:?}");
        let mut socket = answer.socket.take().unwrap_or_else(|| panic!("a 101 carries a socket"));
        assert_eq!(close_frame(&mut socket).await, Some((SUBPROTOCOL_NOT_ACCEPTABLE, "no subprotocol agreed".to_owned())));
        hang_up(socket).await;
    }
    app.stop().await;
}

#[tokio::test]
async fn a_gateway_that_does_not_refuse_proceeds_without_a_subprotocol() {
    let app = start().await;
    let mut answer = upgrade(app.addr, "/lenient", &[("Sec-WebSocket-Protocol", "graphql-ws")]).await;
    assert_eq!(answer.header("sec-websocket-protocol"), None);
    let mut socket = answer.socket.take().unwrap_or_else(|| panic!("expected a 101, got {}", answer.status));
    assert_eq!(which(&mut socket).await, json!({ "id": 1, "data": null }));
    hang_up(socket).await;
    app.stop().await;
}

#[tokio::test]
async fn a_gateway_listing_no_subprotocol_echoes_none() {
    let app = start().await;
    let mut answer = upgrade(app.addr, "/unversioned", &[("Sec-WebSocket-Protocol", "v1.chat")]).await;
    assert_eq!(answer.header("sec-websocket-protocol"), None);
    let mut socket = answer.socket.take().unwrap_or_else(|| panic!("expected a 101, got {}", answer.status));
    assert_eq!(which(&mut socket).await, json!({ "id": 1, "data": null }));
    hang_up(socket).await;
    app.stop().await;
}

#[tokio::test]
async fn refuse_handshake_answers_a_guard_refusal_403_before_the_upgrade() {
    let app = start().await;
    let answer = upgrade(app.addr, "/strict", &[("x-user", "ada")]).await;
    assert_eq!(answer.status, 403, "body: {}", answer.body);
    assert!(answer.socket.is_none());
    app.stop().await;
}

#[tokio::test]
async fn refuse_handshake_answers_an_unauthorized_refusal_401_with_a_challenge() {
    let app = start().await;
    let answer = upgrade(app.addr, "/strict", &[("x-token", "open")]).await;
    assert_eq!(answer.status, 401, "body: {}", answer.body);
    assert_eq!(answer.header("www-authenticate"), Some("Bearer"));
    assert_eq!(answer.body, "who are you?");
    app.stop().await;
}

#[tokio::test]
async fn a_handshake_refusal_counts_against_no_connection_limit() {
    let app = start().await;
    let mut admitted = app.connect("/strict", &[("x-token", "open"), ("x-user", "ada")]).await;
    send_json(&mut admitted, &json!({ "event": "hello", "id": 1 })).await;
    assert_eq!(next_json(&mut admitted).await, json!({ "id": 1, "data": "hello" }));

    // The one slot is taken. A refused upgrade is still refused by its guard, before any slot is
    // asked for, where `refuse = close` would accept it and close it with 1013.
    let refused = upgrade(app.addr, "/strict", &[("x-user", "bob")]).await;
    assert_eq!(refused.status, 403, "body: {}", refused.body);

    // An admitted one over the limit is accepted and closed with 1013.
    let mut over = app.connect("/strict", &[("x-token", "open"), ("x-user", "bob")]).await;
    assert_eq!(close_frame(&mut over).await, Some((1013, "too many connections".to_owned())));
    hang_up(over).await;
    hang_up(admitted).await;
    app.stop().await;
}

#[tokio::test]
async fn connect_guards_by_value_and_by_closure_each_refuse_on_their_own() {
    let app = start().await;
    let all = [("x-value", "yes"), ("x-token", "t2"), ("user-agent", "agent")];
    let mut socket = app.connect("/gated", &all).await;
    send_json(&mut socket, &json!({ "event": "hello", "id": 1 })).await;
    assert_eq!(next_json(&mut socket).await, json!({ "id": 1, "data": "hello" }));
    hang_up(socket).await;

    for missing in 0..all.len() {
        let headers: Vec<(&str, &str)> = all.iter().enumerate().filter(|(at, _)| *at != missing).map(|(_, header)| *header).collect();
        let mut socket = app.connect("/gated", &headers).await;
        let refused = close_frame(&mut socket).await.map(|(code, _)| code);
        assert_eq!(refused, Some(1008), "without {:?} the connection should close with 1008", all[missing]);
        hang_up(socket).await;
    }
    app.stop().await;
}

#[tokio::test]
async fn session_t_starts_from_default_per_connection_and_is_the_one_the_guards_saw() {
    let app = start().await;
    let mut first = app.connect("/counted", &[]).await;
    for expected in [101, 102] {
        send_json(&mut first, &json!({ "event": "bump", "id": 1 })).await;
        assert_eq!(next_json(&mut first).await, json!({ "id": 1, "data": expected }));
    }
    let mut second = app.connect("/counted", &[]).await;
    send_json(&mut second, &json!({ "event": "bump", "id": 1 })).await;
    assert_eq!(next_json(&mut second).await, json!({ "id": 1, "data": 101 }));
    hang_up(first).await;
    hang_up(second).await;
    app.stop().await;
}
