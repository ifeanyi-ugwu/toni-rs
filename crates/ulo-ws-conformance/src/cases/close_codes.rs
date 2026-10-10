//! The close codes a connection ends with: a refused connect by its kind, 1008 for a connect
//! guard, 1013 for `TooManyRequests`, 1011 for a fault or a panic, a code `OnConnect` sets itself,
//! and the drain's 1001. A refused connect is answered with the Close frame alone.

use ulo_ws::DisconnectReason;

use super::Served;
use crate::Host;
use crate::app::{BUSY, BY_GUARD, FAULT, OWN, OWN_CODE, PANIC, REFUSE};

/// The close code and reason a connection to `/refused` refused `how` ends with.
async fn refused_with<H: Host>(app: &Served<H>, how: &str) -> Option<(u16, String)> {
    let mut conn = app.connect("/refused", &[(REFUSE, how)]).await;
    let close = conn.close_frame().await;
    conn.hang_up().await;
    close
}

pub async fn guard_refusal<H: Host>() {
    let app = Served::<H>::start().await;
    assert_eq!(refused_with(&app, BY_GUARD).await.map(|(code, _)| code), Some(1008), "a connect guard's refusal");
    app.stop().await;
}

pub async fn rate_limited<H: Host>() {
    let app = Served::<H>::start().await;
    assert_eq!(refused_with(&app, BUSY).await, Some((1013, "try again later".to_owned())), "`TooManyRequests`");
    app.stop().await;
}

pub async fn faulted<H: Host>() {
    let app = Served::<H>::start().await;
    assert_eq!(refused_with(&app, FAULT).await.map(|(code, _)| code), Some(1011), "`Internal`");
    assert_eq!(refused_with(&app, PANIC).await.map(|(code, _)| code), Some(1011), "a panic in `OnConnect`");
    app.stop().await;
}

pub async fn own_code<H: Host>() {
    let app = Served::<H>::start().await;
    assert_eq!(refused_with(&app, OWN_CODE).await, Some((OWN.0, OWN.1.to_owned())), "`ConnectRefused::code`");
    app.stop().await;
}

pub async fn drain<H: Host>() {
    let app = Served::<H>::start().await;
    let mut conn = app.connect("/echo", &[]).await;
    let closing = app.close_in_background();
    assert_eq!(conn.close_frame().await, Some((1001, "the server is shutting down".to_owned())), "an idle connection at the drain");
    conn.hang_up().await;
    let probe = app.probe.clone();
    app.closed(closing).await;
    assert_eq!(probe.departures.snapshot(), vec![("/echo".to_owned(), DisconnectReason::Drain)], "`on_disconnect` at the drain");
}
