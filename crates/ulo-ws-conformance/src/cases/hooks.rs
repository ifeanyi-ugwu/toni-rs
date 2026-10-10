//! The connection hooks: `OnConnect` refusing a connection, which then never reaches
//! `on_disconnect`, and `on_disconnect` running once per connection that opened.

use serde_json::json;
use ulo_ws::DisconnectReason;

use super::Served;
use crate::Host;
use crate::app::{OWN, OWN_CODE, REFUSE};

pub async fn on_connect_refusal<H: Host>() {
    let app = Served::<H>::start().await;
    let mut refused = app.connect("/refused", &[(REFUSE, OWN_CODE)]).await;
    assert_eq!(refused.close_frame().await, Some((OWN.0, OWN.1.to_owned())), "`OnConnect`'s refusal");
    refused.hang_up().await;

    let mut admitted = app.connect("/refused", &[]).await;
    assert_eq!(admitted.exchange(json!({ "event": "echo", "id": 1, "data": "in" })).await, json!({ "id": 1, "data": "in" }));
    admitted.hang_up().await;
    let probe = app.probe.clone();
    probe.departures.at_least(app.timer(), 1, "the admitted connection's `on_disconnect`").await;
    // The drain waits for every connection's task, so nothing is recorded after it.
    app.stop().await;
    let expected = vec![("/refused".to_owned(), DisconnectReason::ClientClose { code: 1000, reason: String::new() })];
    assert_eq!(probe.departures.snapshot(), expected, "a refused connection reached `on_disconnect`");
}

pub async fn on_disconnect_once<H: Host>() {
    let app = Served::<H>::start().await;
    let mut conn = app.connect("/echo", &[]).await;
    assert_eq!(conn.exchange(json!({ "event": "echo", "id": 1, "data": "ready" })).await, json!({ "id": 1, "data": "ready" }));
    conn.hang_up_with(4001, "bye").await;
    let probe = app.probe.clone();
    probe.departures.at_least(app.timer(), 1, "`on_disconnect`").await;
    app.stop().await;
    let expected = vec![("/echo".to_owned(), DisconnectReason::ClientClose { code: 4001, reason: "bye".to_owned() })];
    assert_eq!(probe.departures.snapshot(), expected, "`on_disconnect` once, with the client's code and reason");
}
