//! What one message gets, past the reply kinds `attributes.rs` covers: the envelope in MessagePack
//! under `codec = msgpack`, a message without an `id`, `cancel` of a streamed answer, and
//! enhancers and `#[meta]` on `#[message]` handlers and on their impl.

mod support;

use std::error::Error;
use std::fmt;
use std::future::ready;

use futures_util::{SinkExt, Stream, StreamExt, stream};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;
use ulo::{BoxError, Dep, ErrorHandler, Guard, Interceptor, Module, ModuleDef, ModuleIdentity, Next, StreamOutcome, injectable, routes};
use ulo_transport::Classify;
use ulo_ws::{Frame, Payload, Reply, Rooms, Ws, WsCx, WsModule};

use support::{Record, Running, Socket, close_frame, hang_up, next_json, next_message, send_json};

/// A handler's own failure, classified `conflict`.
#[derive(Debug, Classify)]
#[classify(conflict)]
struct Taken;

impl fmt::Display for Taken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the name is taken")
    }
}

impl Error for Taken {}

#[derive(Deserialize)]
struct Pair {
    a: i64,
    b: i64,
}

#[derive(Serialize)]
struct Sum {
    sum: i64,
}

/// A gateway speaking MessagePack in binary frames.
#[injectable]
struct Packed;

#[routes]
#[ulo_ws::gateway(path = "/packed", port = own, codec = msgpack)]
impl Packed {
    #[ulo_ws::message("add")]
    fn add(&self, pair: Payload<Pair>) -> Sum {
        Sum { sum: pair.0.a + pair.0.b }
    }

    #[ulo_ws::message("count")]
    fn count(&self, up_to: Payload<u32>) -> impl Stream<Item = Result<u32, Taken>> {
        stream::iter((1..=up_to.0).map(Ok))
    }

    #[ulo_ws::message("fail")]
    fn fail(&self) -> Result<(), Taken> {
        Err(Taken)
    }
}

/// How each streamed answer on the plain gateway ended.
#[derive(Clone)]
struct Ends(Record<StreamOutcome>);

/// A JSON gateway with a streamed answer that never ends on its own, and a broadcast.
#[injectable]
struct Plain {
    ends: Dep<Ends>,
}

#[routes]
#[ulo_ws::gateway(path = "/plain", port = own)]
impl Plain {
    /// One item, then nothing until the answer is cut off.
    #[ulo_ws::message("tail")]
    fn tail(&self, cx: WsCx) -> impl Stream<Item = Result<u32, Taken>> {
        let ends = self.ends.clone();
        cx.exec().on_stream_end(move |outcome| ends.0.push(outcome));
        stream::once(ready(Ok(1))).chain(stream::pending())
    }

    #[ulo_ws::message("echo")]
    fn echo(&self, text: Payload<String>) -> String {
        text.0
    }

    /// To every connection on every gateway, whatever its codec.
    #[ulo_ws::message("shout")]
    async fn shout(&self, data: Payload<Value>, rooms: Dep<Rooms>) -> Result<(), ulo_ws::BroadcastError> {
        rooms.to_all().emit("shout", &data.0).await
    }
}

/// Refuses every call.
struct Deny;

impl Guard<Ws> for Deny {
    async fn can_activate(&self, _cx: &WsCx) -> Result<bool, BoxError> {
        Ok(false)
    }
}

/// One message at a time, so a message's answer is written before the next message is read.
#[injectable]
struct Serial;

#[routes]
#[ulo_ws::gateway(path = "/serial", port = own, max_inflight = 1)]
impl Serial {
    #[ulo_ws::message("greet")]
    fn greet(&self, name: Payload<String>) -> String {
        format!("hello, {}", name.0)
    }

    #[ulo_ws::message("fail")]
    fn fail(&self) -> Result<String, Taken> {
        Err(Taken)
    }

    #[ulo_ws::message("denied")]
    #[guards(value = Deny)]
    fn denied(&self) {}
}

/// A label declared on the gateway's impl and on one of its handlers.
struct Tag(&'static str);

/// Refuses a message on a connection whose upgrade carried `x-banned`. Declared on the impl, so it
/// covers the message handlers and not the connect phase.
struct NotBanned;

impl Guard<Ws> for NotBanned {
    async fn can_activate(&self, cx: &WsCx) -> Result<bool, BoxError> {
        Ok(!cx.conn().head().headers().contains_key("x-banned"))
    }
}

/// Wraps a single answer's data in `{"wrapped": ..}`.
struct Wrap;

impl Interceptor<Ws> for Wrap {
    async fn intercept(&self, cx: &WsCx, next: Next<'_, Ws>) -> Result<Reply, BoxError> {
        let _ = cx;
        match next.run().await? {
            Reply::One(Frame::Text(data)) => Ok(Reply::One(Frame::text(format!("{{\"wrapped\":{data}}}")))),
            other => Ok(other),
        }
    }
}

/// Claims `Taken` with the answer `"recovered"`, leaves any other error as it is.
struct Recover;

impl ErrorHandler<Ws> for Recover {
    async fn handle(&self, err: BoxError, cx: &WsCx) -> Result<Reply, BoxError> {
        let _ = cx;
        let taken = err.downcast_ref::<ulo_transport::CallError>().and_then(|error| error.source_as::<Taken>()).is_some();
        if taken { Ok(Reply::One(Frame::text("\"recovered\""))) } else { Err(err) }
    }
}

#[injectable]
struct Enhanced;

#[routes]
#[ulo_ws::gateway(path = "/enhanced", port = own)]
#[guards(ws(value = NotBanned))]
#[meta(Tag("gateway"))]
impl Enhanced {
    #[ulo_ws::message("open")]
    fn open(&self) -> &'static str {
        "open"
    }

    #[ulo_ws::message("secret")]
    #[guards(value = Deny)]
    fn secret(&self) -> &'static str {
        "secret"
    }

    #[ulo_ws::message("wrapped")]
    #[interceptors(value = Wrap)]
    fn wrapped(&self) -> u32 {
        7
    }

    #[ulo_ws::message("risky")]
    #[error_handlers(value = Recover)]
    fn risky(&self, fail: Payload<bool>) -> Result<&'static str, Taken> {
        if fail.0 { Err(Taken) } else { Ok("fine") }
    }

    /// The labels this handler's metadata carries, most specific first.
    #[ulo_ws::message("tagged")]
    #[meta(Tag("method"))]
    fn tagged(&self, cx: WsCx) -> Vec<&'static str> {
        labels(&cx)
    }

    #[ulo_ws::message("untagged")]
    fn untagged(&self, cx: WsCx) -> Vec<&'static str> {
        labels(&cx)
    }
}

/// `meta::<Tag>()` first, then every `Tag` declared, from the handler information `dispatch` set.
fn labels(cx: &WsCx) -> Vec<&'static str> {
    let Some(info) = cx.exec().handler() else { return vec!["no handler information"] };
    let first = info.meta::<Tag>().map_or("none", |tag| tag.0);
    std::iter::once(first).chain(info.meta_all::<Tag>().map(|tag| tag.0)).collect()
}

struct Root {
    ends: Ends,
}

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(WsModule::for_root());
        m.value(self.ends.clone());
        m.controller::<Packed>();
        m.controller::<Plain>();
        m.controller::<Serial>();
        m.controller::<Enhanced>();
    }
}

async fn start() -> (Running, Ends) {
    let ends = Ends(Record::new());
    let app = Running::start(Root { ends: ends.clone() }, ulo_ws_hyper::Server::new("127.0.0.1:0")).await;
    (app, ends)
}

async fn send_packed(socket: &mut Socket, message: &Value) {
    let bytes = rmp_serde::to_vec_named(message).unwrap_or_else(|error| panic!("{message} does not encode: {error}"));
    socket.send(Message::binary(bytes)).await.unwrap_or_else(|error| panic!("sending {message} failed: {error}"));
}

async fn next_packed(socket: &mut Socket) -> Value {
    match next_message(socket).await {
        Message::Binary(bytes) => {
            rmp_serde::from_slice(&bytes).unwrap_or_else(|error| panic!("the server sent bytes that are not MessagePack: {error}"))
        }
        other => panic!("expected a binary message, got {other:?}"),
    }
}

async fn exchange(socket: &mut Socket, message: Value) -> Value {
    send_json(socket, &message).await;
    next_json(socket).await
}

fn conflict(id: Option<Value>) -> Value {
    let error = json!({ "kind": "conflict", "message": "the name is taken", "details": [] });
    match id {
        Some(id) => json!({ "id": id, "error": error }),
        None => json!({ "error": error }),
    }
}

#[tokio::test]
async fn msgpack_carries_the_envelope_and_its_answers_in_binary_frames() {
    let (app, _) = start().await;
    let mut socket = app.connect("/packed", &[]).await;

    send_packed(&mut socket, &json!({ "event": "add", "id": 7, "data": { "a": 2, "b": 3 } })).await;
    assert_eq!(next_packed(&mut socket).await, json!({ "id": 7, "data": { "sum": 5 } }));

    send_packed(&mut socket, &json!({ "event": "count", "id": "s", "data": 2 })).await;
    for expected in [json!({ "id": "s", "data": 1 }), json!({ "id": "s", "data": 2 }), json!({ "id": "s", "complete": true })] {
        assert_eq!(next_packed(&mut socket).await, expected);
    }

    send_packed(&mut socket, &json!({ "event": "fail", "id": 8 })).await;
    assert_eq!(next_packed(&mut socket).await, conflict(Some(json!(8))));
    hang_up(socket).await;
    app.stop().await;
}

#[tokio::test]
async fn a_text_frame_on_a_msgpack_gateway_closes_with_1003() {
    let (app, _) = start().await;
    let mut socket = app.connect("/packed", &[]).await;
    send_json(&mut socket, &json!({ "event": "add", "id": 1, "data": { "a": 1, "b": 1 } })).await;
    assert_eq!(close_frame(&mut socket).await, Some((1003, "this gateway reads binary frames".to_owned())));
    hang_up(socket).await;
    app.stop().await;
}

#[tokio::test]
async fn a_broadcast_reaches_each_gateway_in_its_own_codec() {
    let (app, _) = start().await;
    let mut packed = app.connect("/packed", &[]).await;
    let mut plain = app.connect("/plain", &[]).await;
    send_json(&mut plain, &json!({ "event": "shout", "data": { "n": 1 } })).await;
    assert_eq!(next_json(&mut plain).await, json!({ "event": "shout", "data": { "n": 1 } }));
    assert_eq!(next_packed(&mut packed).await, json!({ "event": "shout", "data": { "n": 1 } }));
    hang_up(packed).await;
    hang_up(plain).await;
    app.stop().await;
}

#[tokio::test]
async fn a_message_without_an_id_is_answered_only_when_it_fails() {
    let (app, _) = start().await;
    let mut socket = app.connect("/serial", &[]).await;
    // Each fire-and-forget message is followed by one with an id: the gateway reads one message at
    // a time, so an ack for the first would arrive before the second's answer.
    send_json(&mut socket, &json!({ "event": "greet", "data": "ada" })).await;
    assert_eq!(exchange(&mut socket, json!({ "event": "greet", "id": 1, "data": "bob" })).await, json!({ "id": 1, "data": "hello, bob" }));

    send_json(&mut socket, &json!({ "event": "fail" })).await;
    assert_eq!(next_json(&mut socket).await, conflict(None));

    send_json(&mut socket, &json!({ "event": "denied" })).await;
    let refused = next_json(&mut socket).await;
    assert_eq!(refused.get("id"), None, "a refusal without an id: {refused}");
    assert_eq!(refused["error"]["kind"], json!("forbidden"), "{refused}");

    send_json(&mut socket, &json!({ "event": "nothing.handles.this" })).await;
    let unhandled = next_json(&mut socket).await;
    assert_eq!(unhandled.get("id"), None, "an unhandled event without an id: {unhandled}");
    assert_eq!(unhandled["error"]["kind"], json!("unimplemented"), "{unhandled}");
    hang_up(socket).await;
    app.stop().await;
}

#[tokio::test]
async fn cancel_cuts_off_a_streamed_answer_as_client_cancelled() {
    let (app, ends) = start().await;
    let mut socket = app.connect("/plain", &[]).await;
    send_json(&mut socket, &json!({ "event": "tail", "id": "t" })).await;
    assert_eq!(next_json(&mut socket).await, json!({ "id": "t", "data": 1 }));

    send_json(&mut socket, &json!({ "event": "cancel", "id": "t" })).await;
    let outcomes = ends.0.at_least(1, "the cancelled stream's end").await;
    assert_eq!(outcomes, vec![StreamOutcome::CutOff(Some(ulo::CancelReason::ClientCancelled))]);
    // Nothing more is written for the cancelled message, not even `complete`: the next frame is
    // the next message's answer.
    assert_eq!(exchange(&mut socket, json!({ "event": "echo", "id": 2, "data": "next" })).await, json!({ "id": 2, "data": "next" }));
    hang_up(socket).await;
    app.stop().await;
}

#[tokio::test]
async fn cancel_without_an_id_is_a_bad_request() {
    let (app, _) = start().await;
    let mut socket = app.connect("/plain", &[]).await;
    let answer = exchange(&mut socket, json!({ "event": "cancel" })).await;
    assert_eq!(answer.get("id"), None, "{answer}");
    assert_eq!(answer["error"]["kind"], json!("bad_request"), "{answer}");
    hang_up(socket).await;
    app.stop().await;
}

#[tokio::test]
async fn method_guards_interceptors_and_error_handlers_apply_to_their_message() {
    let (app, _) = start().await;
    let mut socket = app.connect("/enhanced", &[]).await;
    assert_eq!(exchange(&mut socket, json!({ "event": "open", "id": 1 })).await, json!({ "id": 1, "data": "open" }));

    let refused = exchange(&mut socket, json!({ "event": "secret", "id": 2 })).await;
    assert_eq!((refused["id"].clone(), refused["error"]["kind"].clone()), (json!(2), json!("forbidden")), "{refused}");

    assert_eq!(exchange(&mut socket, json!({ "event": "wrapped", "id": 3 })).await, json!({ "id": 3, "data": { "wrapped": 7 } }));

    assert_eq!(exchange(&mut socket, json!({ "event": "risky", "id": 4, "data": false })).await, json!({ "id": 4, "data": "fine" }));
    assert_eq!(exchange(&mut socket, json!({ "event": "risky", "id": 5, "data": true })).await, json!({ "id": 5, "data": "recovered" }));
    hang_up(socket).await;
    app.stop().await;
}

#[tokio::test]
async fn an_impl_guard_covers_every_message_and_not_the_connect_phase() {
    let (app, _) = start().await;
    let mut banned = app.connect("/enhanced", &[("x-banned", "1")]).await;
    for (id, event) in [(1, "open"), (2, "wrapped"), (3, "tagged")] {
        let refused = exchange(&mut banned, json!({ "event": event, "id": id })).await;
        assert_eq!((refused["id"].clone(), refused["error"]["kind"].clone()), (json!(id), json!("forbidden")), "{event}: {refused}");
    }
    hang_up(banned).await;
    app.stop().await;
}

#[tokio::test]
async fn meta_on_a_handler_is_more_specific_than_meta_on_its_impl() {
    let (app, _) = start().await;
    let mut socket = app.connect("/enhanced", &[]).await;
    assert_eq!(exchange(&mut socket, json!({ "event": "tagged", "id": 1 })).await, json!({ "id": 1, "data": ["method", "method", "gateway"] }));
    assert_eq!(exchange(&mut socket, json!({ "event": "untagged", "id": 2 })).await, json!({ "id": 2, "data": ["gateway", "gateway"] }));
    hang_up(socket).await;
    app.stop().await;
}
