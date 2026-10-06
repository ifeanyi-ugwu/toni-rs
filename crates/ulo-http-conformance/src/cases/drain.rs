//! Scenarios: drain.

use ulo::Signal;

use crate::wire::{Exchange, PATIENCE, Raw, has_header, start, status_of};
use crate::{Host, Mode};

/// The drain: a request still arriving once the app has stopped admitting executions is answered
/// 503, and the connection closes after it, by `Connection: close` on HTTP/1.1 and GOAWAY on
/// HTTP/2 where the host serves HTTP/2 without TLS.
pub async fn drain<H: Host>(mode: Mode) {
    http1::<H>(mode).await;
    http2::<H>(mode).await;
}

/// The request's head is half written before the shutdown, which keeps the connection busy
/// through the host's graceful stop, and finished once the drain has begun.
///
/// A host that closes its listener when the drain begins resets a connection still in the
/// backlog. A request answered on a second connection opened after this one shows the host
/// has accepted this one, since a listener hands connections over in the order they arrived.
async fn http1<H: Host>(mode: Mode) {
    let host = start::<H>(mode).await;
    let mut raw = Raw::connect(&host.authority()).await;
    raw.write(format!("GET {} HTTP/1.1\r\nHost: suite\r\n", host.target("/hit")).as_bytes()).await;
    let accepted = host.send(Exchange::get("/hit")).await;
    assert_eq!(accepted.status, 200, "the request proving the first connection was accepted");
    let app = host.app.clone();
    let closing = tokio::spawn(async move { app.close(Signal::new("suite")).await });
    tokio::time::timeout(PATIENCE, host.app.draining()).await.expect("the drain begins");
    raw.write(b"\r\n").await;
    let answer = raw.read_to_close().await.expect("the host closes the connection after its answer");
    assert_eq!(status_of(&answer), Some(503), "a request during the drain: {}", String::from_utf8_lossy(&answer));
    assert!(has_header(&answer, "connection", "close"), "the drain's 503 carries `Connection: close`");
    let _ = closing.await;
    host.stop().await;
}

/// A long-lived stream holds an HTTP/2 connection open through the drain; a new stream on it is
/// then not served. A host that does not speak HTTP/2 without TLS skips this shape.
async fn http2<H: Host>(mode: Mode) {
    let host = start::<H>(mode).await;
    let Ok(client) = reqwest::Client::builder().http2_prior_knowledge().timeout(PATIENCE).build() else {
        host.stop().await;
        return;
    };
    let speaks_h2 = client.get(host.url("/hit")).send().await.is_ok_and(|response| response.status().as_u16() == 200);
    if !speaks_h2 {
        host.stop().await;
        return;
    }
    let held = client.get(host.url("/endless")).send().await.expect("the stream opens");
    let app = host.app.clone();
    let closing = tokio::spawn(async move { app.close(Signal::new("suite")).await });
    tokio::time::timeout(PATIENCE, host.app.draining()).await.expect("the drain begins");
    let during = client.request(Exchange::get("/hit").method, host.url("/hit")).send().await;
    if let Ok(response) = during {
        assert_eq!(response.status().as_u16(), 503, "an HTTP/2 request during the drain was served");
    }
    drop(held);
    let _ = closing.await;
    host.stop().await;
}
