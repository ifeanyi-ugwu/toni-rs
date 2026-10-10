//! One message through the envelope: a round trip, the `error` envelope for an event nothing
//! handles and for a frame naming no event, a control frame answered by the protocol layer alone,
//! `message_limit` counted after reassembly, and a streamed answer written to its end.

use async_tungstenite::tungstenite::Message;
use async_tungstenite::tungstenite::protocol::frame::Frame as WireFrame;
use async_tungstenite::tungstenite::protocol::frame::coding::{Data, OpCode};
use serde_json::{Value, json};
use ulo_ws::DisconnectReason;

use super::Served;
use crate::Host;
use crate::app::MESSAGE_LIMIT;

/// `answer` is the `error` envelope of kind `kind`, carrying `id` or none.
fn error_envelope(what: &str, answer: &Value, id: Option<Value>, kind: &str) {
    assert_eq!(answer.get("id"), id.as_ref(), "{what}: {answer}");
    assert_eq!(answer["error"]["kind"], json!(kind), "{what}: {answer}");
    assert!(answer["error"]["message"].is_string(), "{what}: the envelope carries no message: {answer}");
}

pub async fn round_trip<H: Host>() {
    let app = Served::<H>::start().await;
    let mut conn = app.connect("/echo", &[]).await;
    assert_eq!(conn.exchange(json!({ "event": "echo", "id": 1, "data": "hello" })).await, json!({ "id": 1, "data": "hello" }));
    let named = conn.exchange(json!({ "event": "echo", "id": "a-string-id", "data": "again" })).await;
    assert_eq!(named, json!({ "id": "a-string-id", "data": "again" }), "an `id` is echoed as sent");
    conn.hang_up().await;
    app.stop().await;
}

pub async fn unknown_event<H: Host>() {
    let app = Served::<H>::start().await;
    let mut conn = app.connect("/echo", &[]).await;
    let answer = conn.exchange(json!({ "event": "nothing.handles.this", "id": 2 })).await;
    error_envelope("an event nothing handles", &answer, Some(json!(2)), "unimplemented");
    conn.send_json(&json!({ "event": "nothing.handles.this" })).await;
    error_envelope("the same event without an id", &conn.next_json().await, None, "unimplemented");
    assert_eq!(conn.exchange(json!({ "event": "echo", "id": 3, "data": "still" })).await, json!({ "id": 3, "data": "still" }));
    conn.hang_up().await;
    app.stop().await;
}

pub async fn no_event<H: Host>() {
    let app = Served::<H>::start().await;
    let mut conn = app.connect("/echo", &[]).await;
    let answer = conn.exchange(json!({ "id": 4, "data": "no event" })).await;
    error_envelope("a frame naming no event", &answer, Some(json!(4)), "bad_request");
    conn.send(Message::text("not json")).await;
    error_envelope("a frame that is not JSON", &conn.next_json().await, None, "bad_request");
    assert_eq!(conn.exchange(json!({ "event": "echo", "id": 5, "data": "still" })).await, json!({ "id": 5, "data": "still" }));
    conn.hang_up().await;
    app.stop().await;
}

/// A Ping is answered with its Pong and nothing else, and an unsolicited Pong with nothing: the
/// next data frame is the answer to the message sent after them.
pub async fn control_frame<H: Host>() {
    let app = Served::<H>::start().await;
    let mut conn = app.connect("/echo", &[]).await;
    conn.send(Message::Ping(b"are you there".to_vec().into())).await;
    conn.send(Message::Pong(b"unasked".to_vec().into())).await;
    conn.send_json(&json!({ "event": "echo", "id": 6, "data": "after" })).await;
    let mut seen = Vec::new();
    loop {
        match conn.next_raw("the answers to a Ping, a Pong and a message").await {
            Some(Ok(Message::Text(text))) => {
                let answer: Value = serde_json::from_str(&text).unwrap_or_else(|error| panic!("{text:?} is not JSON: {error}"));
                assert_eq!(answer, json!({ "id": 6, "data": "after" }), "the first data frame; before it: {seen:?}");
                break;
            }
            Some(Ok(message)) => seen.push(message),
            other => panic!("expected the answers, got {other:?}; before it: {seen:?}"),
        }
    }
    assert_eq!(seen, vec![Message::Pong(b"are you there".to_vec().into())], "what the server wrote before the message's answer");
    conn.hang_up().await;
    app.stop().await;
}

/// Two fragments, each under the 128-byte limit and together over it, close the connection with
/// 1009 as a protocol error.
pub async fn message_limit<H: Host>() {
    let app = Served::<H>::start().await;
    let mut conn = app.connect("/small", &[]).await;
    assert_eq!(conn.exchange(json!({ "event": "echo", "id": 1, "data": "short" })).await, json!({ "id": 1, "data": "short" }));
    let message = json!({ "event": "echo", "id": 2, "data": "y".repeat(MESSAGE_LIMIT + 12) }).to_string();
    let (first, rest) = message.split_at(80);
    assert!(first.len() < MESSAGE_LIMIT && rest.len() < MESSAGE_LIMIT && message.len() > MESSAGE_LIMIT);
    conn.send(Message::Frame(WireFrame::message(first.as_bytes().to_vec(), OpCode::Data(Data::Text), false))).await;
    conn.send(Message::Frame(WireFrame::message(rest.as_bytes().to_vec(), OpCode::Data(Data::Continue), true))).await;
    assert_eq!(conn.close_frame().await.map(|(code, _)| code), Some(1009), "a message over `message_limit`");
    conn.hang_up().await;
    let ends = app.probe.departures.at_least(app.timer(), 1, "the oversized connection's `on_disconnect`").await;
    assert_eq!(ends, vec![("/small".to_owned(), DisconnectReason::ProtocolError)]);
    app.stop().await;
}

pub async fn reply_stream<H: Host>() {
    let app = Served::<H>::start().await;
    let mut conn = app.connect("/echo", &[]).await;
    conn.send_json(&json!({ "event": "count", "id": "s", "data": 5 })).await;
    for n in 1..=5 {
        assert_eq!(conn.next_json().await, json!({ "id": "s", "data": n }), "item {n} of the stream");
    }
    assert_eq!(conn.next_json().await, json!({ "id": "s", "complete": true }), "the stream's end");
    assert_eq!(conn.exchange(json!({ "event": "echo", "id": 7, "data": "after" })).await, json!({ "id": 7, "data": "after" }));
    conn.hang_up().await;
    app.stop().await;
}
