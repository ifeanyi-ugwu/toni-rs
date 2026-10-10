//! A gateway's limits: `max_connections` closing the next connection with 1013, `max_inflight`
//! of one holding the next message unread, and `max_outbound` of one closing a slow consumer while
//! a streamed answer waits for room instead.

use serde_json::json;
use ulo_ws::DisconnectReason;

use super::Served;
use crate::Host;

/// How many frames the slow-consumer burst queues without yielding: far more than a server can
/// write between two of them, so the one place overflows on any executor.
const BURST: u32 = 1_000;

pub async fn max_connections<H: Host>() {
    let app = Served::<H>::start().await;
    let mut first = app.connect("/small", &[]).await;
    assert_eq!(first.exchange(json!({ "event": "echo", "id": 1, "data": "first" })).await, json!({ "id": 1, "data": "first" }));
    let mut second = app.connect("/small", &[]).await;
    assert_eq!(second.close_frame().await, Some((1013, "too many connections".to_owned())), "the connection over the limit");
    second.hang_up().await;
    let still = first.exchange(json!({ "event": "echo", "id": 2, "data": "still here" })).await;
    assert_eq!(still, json!({ "id": 2, "data": "still here" }), "the admitted connection after the refusal");
    first.hang_up().await;
    app.stop().await;
}

/// With `hold` in flight, `open` is sent and not read: a round trip on a second connection, which
/// has its own message to spend, completes first, and `open` runs only once the scenario opens the
/// gate and `hold` has answered.
pub async fn max_inflight<H: Host>() {
    let app = Served::<H>::start().await;
    let gate = app.probe.gate.clone();
    let mut conn = app.connect("/serial", &[]).await;
    conn.send_json(&json!({ "event": "hold", "id": 1 })).await;
    gate.log.at_least(app.timer(), 1, "`hold` starting").await;
    conn.send_json(&json!({ "event": "open", "id": 2 })).await;
    let mut other = app.connect("/serial", &[]).await;
    assert_eq!(other.exchange(json!({ "event": "noop", "id": 9 })).await, json!({ "id": 9, "complete": true }));
    gate.open("opened by the scenario");

    assert_eq!(conn.next_json().await, json!({ "id": 1, "data": "held" }));
    assert_eq!(conn.next_json().await, json!({ "id": 2, "data": "opened" }));
    assert_eq!(gate.log.snapshot(), vec!["hold started", "opened by the scenario", "hold done", "opened by a message"]);
    conn.hang_up().await;
    other.hang_up().await;
    app.stop().await;
}

pub async fn max_outbound<H: Host>() {
    let app = Served::<H>::start().await;
    let mut conn = app.connect("/outbound", &[]).await;
    conn.send_json(&json!({ "event": "burst", "id": 1, "data": BURST })).await;
    let (close, data) = conn.close_frame_after_data().await;
    assert_eq!(close, Some((1008, "slow consumer".to_owned())), "an outbound queue over `max_outbound`, after {data} data frames");
    assert!(data < BURST as usize, "every frame of the burst was written: {data}");
    conn.hang_up().await;
    let ends = app.probe.departures.at_least(app.timer(), 1, "the slow consumer's `on_disconnect`").await;
    assert_eq!(ends, vec![("/outbound".to_owned(), DisconnectReason::ServerClose { code: 1008 })]);
    app.stop().await;
}

pub async fn max_outbound_stream<H: Host>() {
    let app = Served::<H>::start().await;
    let mut conn = app.connect("/outbound", &[]).await;
    conn.send_json(&json!({ "event": "count", "id": 1, "data": 50 })).await;
    for n in 1..=50 {
        assert_eq!(conn.next_json().await, json!({ "id": 1, "data": n }), "item {n} of the stream");
    }
    assert_eq!(conn.next_json().await, json!({ "id": 1, "complete": true }));
    let after = conn.exchange(json!({ "event": "echo", "id": 2, "data": "after" })).await;
    assert_eq!(after, json!({ "id": 2, "data": "after" }), "the connection outlived the stream");
    conn.hang_up().await;
    let ends = app.probe.departures.at_least(app.timer(), 1, "the connection's `on_disconnect`").await;
    assert_eq!(ends, vec![("/outbound".to_owned(), DisconnectReason::ClientClose { code: 1000, reason: String::new() })]);
    app.stop().await;
}
