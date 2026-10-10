//! `on_stream_end` reports the reply the server writes: a written stream reports `Completed`, and
//! a stream an interceptor discards was never the reply and reports nothing. Each execution's
//! callback records that the execution ended when it is dropped, so a scenario reads what was
//! reported once nothing more can be.

use serde_json::{Value, json};
use ulo::StreamOutcome;

use super::Served;
use crate::Host;
use crate::app::{DISCARDED, StreamReport, WRITTEN};

/// Sends `event` and reads `frames` frames of its reply; answers them and what its execution
/// reported, in order, up to and including its end.
async fn call<H: Host>(app: &Served<H>, event: &'static str, frames: usize) -> (Vec<Value>, Vec<StreamReport>) {
    let mut conn = app.connect("/streams", &[]).await;
    conn.send_json(&json!({ "event": event, "id": "m" })).await;
    let mut written = Vec::new();
    for _ in 0..frames {
        written.push(conn.next_json().await);
    }
    let what = format!("the end of `{event}`'s execution");
    let reports = app.probe.streams.until(app.timer(), &what, |reports| reports.contains(&StreamReport::Ended(event))).await;
    conn.hang_up().await;
    (written, reports)
}

pub async fn written<H: Host>() {
    let app = Served::<H>::start().await;
    let (frames, reports) = call(&app, WRITTEN, 3).await;
    let expected = vec![json!({ "id": "m", "data": 1 }), json!({ "id": "m", "data": 2 }), json!({ "id": "m", "complete": true })];
    assert_eq!(frames, expected, "the stream was not written to its end");
    assert_eq!(reports, vec![StreamReport::Outcome(WRITTEN, StreamOutcome::Completed), StreamReport::Ended(WRITTEN)]);
    app.stop().await;
}

pub async fn discarded<H: Host>() {
    let app = Served::<H>::start().await;
    let (frames, reports) = call(&app, DISCARDED, 1).await;
    assert_eq!(frames, vec![json!({ "id": "m", "data": 0 })], "the interceptor's answer was not the reply");
    assert_eq!(reports, vec![StreamReport::Ended(DISCARDED)], "the stream the interceptor discarded reported its end");
    app.stop().await;
}
