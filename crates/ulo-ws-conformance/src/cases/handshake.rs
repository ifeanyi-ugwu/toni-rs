//! The upgrade handshake: the 101 and its `Sec-WebSocket-Accept`, the refusals RFC 6455 §4.2.1
//! names and the drain's 503, the subprotocol the 101 echoes, and the connection phase refusing
//! before the upgrade under `refuse = handshake`.
//!
//! A request that carries no `Upgrade` header at all is no upgrade request: on the HTTP server's
//! port the HTTP application answers it, so no scenario sends one. Each refusal here keeps the
//! header and spoils something else.

use serde_json::json;
use ulo_ws::Port;

use super::Served;
use crate::Host;
use crate::app::{TOKEN, USER, WHO};
use crate::client::{Answer, Request, read_answer, write};

/// RFC 6455 §1.3's sample key and the accept value it derives.
const RFC_KEY: &str = "dGhlIHNhbXBsZSBub25jZQ==";
const RFC_ACCEPT: &str = "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=";

/// The type every handshake refusal's reason is written in.
const PLAIN: &str = "text/plain; charset=utf-8";

/// `answer` is a handshake refusal with `status`: the reason as a plain-text body, not empty.
fn refused<S>(what: &str, answer: &Answer<S>, status: u16) {
    assert_eq!(answer.status, status, "{what}: {}", answer.body);
    assert!(answer.socket.is_none(), "{what}: refused, yet switched protocols");
    assert_eq!(answer.header("content-type"), Some(PLAIN), "{what}");
    assert!(!answer.body.is_empty(), "{what}: the refusal carries no reason");
}

pub async fn switches<H: Host>() {
    let app = Served::<H>::start().await;
    let answer = app.upgrade(&Request::upgrade("/echo").key(RFC_KEY)).await;
    assert_eq!(answer.status, 101, "a valid upgrade request: {}", answer.body);
    assert_eq!(answer.header("sec-websocket-accept"), Some(RFC_ACCEPT), "RFC 6455 §1.3's sample key");
    assert!(answer.header("upgrade").is_some_and(|value| value.eq_ignore_ascii_case("websocket")), "{:?}", answer.headers);
    assert!(answer.header("connection").is_some_and(|value| value.eq_ignore_ascii_case("upgrade")), "{:?}", answer.headers);
    assert_eq!(answer.header("sec-websocket-protocol"), None, "a subprotocol nobody offered");
    let mut conn = app.conn(answer, "/echo");
    assert_eq!(conn.exchange(json!({ "event": "echo", "id": 1, "data": "switched" })).await, json!({ "id": 1, "data": "switched" }));
    conn.hang_up().await;
    app.stop().await;
}

pub async fn no_gateway<H: Host>() {
    let app = Served::<H>::start().await;
    let answer = app.upgrade(&Request::upgrade("/nowhere")).await;
    // The status alone: on the HTTP server's port a path off the gateways is the HTTP
    // application's, which answers its own 404.
    assert_eq!(answer.status, 404, "an upgrade to a path no gateway serves: {}", answer.body);
    assert!(answer.socket.is_none());
    app.stop().await;
}

pub async fn method<H: Host>() {
    let app = Served::<H>::start().await;
    let answer = app.upgrade(&Request::upgrade("/echo").method("POST").header("Content-Length", "0")).await;
    refused("a POST", &answer, 405);
    assert_eq!(answer.header("allow"), Some("GET"), "a 405 names the method a handshake takes");
    app.stop().await;
}

pub async fn malformed<H: Host>() {
    let app = Served::<H>::start().await;
    let cases = [
        ("HTTP/1.0", Request::upgrade("/echo").version("HTTP/1.0")),
        ("an upgrade to another protocol", Request::upgrade("/echo").set("Upgrade", "h2c")),
        ("no `Connection: Upgrade`", Request::upgrade("/echo").set("Connection", "keep-alive")),
        ("no `Sec-WebSocket-Key`", Request::upgrade("/echo").without("Sec-WebSocket-Key")),
    ];
    for (what, request) in cases {
        refused(what, &app.upgrade(&request).await, 400);
    }
    app.stop().await;
}

pub async fn version<H: Host>() {
    let app = Served::<H>::start().await;
    let answer = app.upgrade(&Request::upgrade("/echo").set("Sec-WebSocket-Version", "12")).await;
    refused("version 12", &answer, 426);
    assert_eq!(answer.header("sec-websocket-version"), Some("13"), "a 426 names the version the server speaks");
    app.stop().await;
}

/// The drain refuses an upgrade 503. A server stops accepting connections when its drain begins,
/// and drops those still queued unaccepted, so the request travels on a connection the server
/// accepted before it: one opened before the idle WebSocket connection whose 101 then arrives,
/// since one listener accepts in arrival order. Its head is left unfinished until that idle
/// connection's 1001 shows the drain has begun.
///
/// A standalone server writes the handshake's own refusal. On the HTTP server's port the HTTP
/// server's drain answers every request before any upgrade handler sees it, with its own 503
/// document, so there the status alone is asserted, as for a path without a gateway.
pub async fn draining<H: Host>() {
    let app = Served::<H>::start().await;
    let request = Request::upgrade("/echo");
    let head = request.head(&app.host());
    let (start, rest) = head.split_at(head.find("\r\n").map_or(0, |line| line + 2));
    let mut pending = app.stream().await;
    write(&mut pending, start).await;
    let mut idle = app.connect("/echo", &[]).await;

    let closing = app.close_in_background();
    assert_eq!(idle.close_frame().await, Some((1001, "server shutting down".to_owned())), "the drain's close");
    write(&mut pending, rest).await;
    let answer = crate::app::within(app.timer(), "the answer to an upgrade request finished during the drain", read_answer(pending, request.sent_key())).await;
    match H::PORT {
        Port::Own => refused("an upgrade request during the drain", &answer, 503),
        Port::Http => {
            assert_eq!(answer.status, 503, "an upgrade request during the drain: {}", answer.body);
            assert!(answer.socket.is_none(), "an upgrade request during the drain switched protocols");
        }
    }
    idle.hang_up().await;
    app.closed(closing).await;
}

pub async fn subprotocol<H: Host>() {
    let app = Served::<H>::start().await;
    let cases: [(&str, &[&str], Option<&str>); 5] = [
        ("both offered, the gateway's first listed", &["v1.chat, v2.chat"], Some("v2.chat")),
        ("one offered", &["v1.chat"], Some("v1.chat")),
        ("an unknown one offered first", &["other, v1.chat"], Some("v1.chat")),
        ("offers over two header lines", &["other", "v1.chat"], Some("v1.chat")),
        ("none the gateway lists", &["graphql-ws"], None),
    ];
    for (what, offers, chosen) in cases {
        let request = offers.iter().fold(Request::upgrade("/versioned"), |request, offer| request.header("Sec-WebSocket-Protocol", offer));
        let answer = app.upgrade(&request).await;
        assert_eq!(answer.status, 101, "{what}: {}", answer.body);
        assert_eq!(answer.header("sec-websocket-protocol"), chosen, "{what}");
        let mut conn = app.conn(answer, "/versioned");
        let which = conn.exchange(json!({ "event": "which", "id": 1 })).await;
        assert_eq!(which, json!({ "id": 1, "data": chosen }), "{what}: the subprotocol the connection's `UpgradeHead` carries");
        conn.hang_up().await;
    }
    app.stop().await;
}

pub async fn refused_before_upgrade<H: Host>() {
    let app = Served::<H>::start().await;
    let forbidden = app.upgrade(&Request::upgrade("/strict").header(USER, "ada")).await;
    refused("a connect guard's refusal", &forbidden, 403);

    let unauthorized = app.upgrade(&Request::upgrade("/strict").header(TOKEN.0, TOKEN.1)).await;
    refused("`OnConnect`'s `Unauthorized`", &unauthorized, 401);
    assert_eq!(unauthorized.header("www-authenticate"), Some("Bearer"), "a 401 carries its challenge");
    assert_eq!(unauthorized.body, WHO, "the refusal's own reason");

    let mut admitted = app.connect("/strict", &[TOKEN, (USER, "ada")]).await;
    assert_eq!(admitted.exchange(json!({ "event": "echo", "id": 1, "data": "in" })).await, json!({ "id": 1, "data": "in" }));
    admitted.hang_up().await;
    app.stop().await;
}
