//! Keep-alive on `/heartbeat`: a Ping every 100 ms, and 150 ms for each Pong. The client answers a
//! Ping on its next read, so a scenario that stops reading withholds the Pong.

use async_tungstenite::tungstenite::Message;
use serde_json::json;
use ulo_ws::DisconnectReason;

use super::Served;
use crate::Host;

/// Three Pings arrive, each answered as the next read begins, over longer than one Pong may take,
/// and the connection still answers.
pub async fn pings<H: Host>() {
    let app = Served::<H>::start().await;
    let mut conn = app.connect("/heartbeat", &[]).await;
    let mut pings = 0;
    while pings < 3 {
        match conn.next_raw("a keep-alive Ping").await {
            Some(Ok(Message::Ping(_))) => pings += 1,
            other => panic!("expected a Ping, got {other:?}"),
        }
    }
    assert_eq!(conn.exchange(json!({ "event": "echo", "id": 1, "data": "alive" })).await, json!({ "id": 1, "data": "alive" }));
    assert_eq!(app.probe.departures.snapshot(), Vec::new(), "the connection ended while its client answered every Ping");
    conn.hang_up().await;
    app.stop().await;
}

/// The first Ping is read and never answered: the connection ends as `Lost`, dropped without a
/// Close frame.
pub async fn pong_timeout<H: Host>() {
    let app = Served::<H>::start().await;
    let mut conn = app.connect("/heartbeat", &[]).await;
    let ping = conn.next_raw("the first keep-alive Ping").await;
    assert!(matches!(ping, Some(Ok(Message::Ping(_)))), "expected a Ping, got {ping:?}");
    let ends = app.probe.departures.at_least(app.timer(), 1, "the end of a connection whose Pong never came").await;
    assert_eq!(ends, vec![("/heartbeat".to_owned(), DisconnectReason::Lost)]);
    let ending = loop {
        match conn.next_raw("the connection's end").await {
            Some(Ok(Message::Ping(_))) => continue,
            other => break other,
        }
    };
    assert!(!matches!(ending, Some(Ok(Message::Close(_)))), "a lost connection is dropped, not closed: {ending:?}");
    app.stop().await;
}
