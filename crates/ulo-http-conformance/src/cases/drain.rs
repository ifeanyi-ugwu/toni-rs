//! Scenarios: drain. A request still arriving once the app has stopped admitting executions is
//! answered 503, and the connection closes after it, by `Connection: close` on HTTP/1.1 and
//! GOAWAY on HTTP/2.

use std::time::Duration;

use bytes::Bytes;
use tokio::net::TcpStream;
use ulo::Signal;
use ulo_http::embed::{DrainAbandoned, DrainPending};

use crate::wire::{Exchange, PATIENCE, Raw, Running, has_header, not_a_timeout, start, status_of};
use crate::{Host, Mode};

/// How long after the drain begins a host has to apply its graceful stop to its connections
/// before the scenario finishes a request head. actix's stop reaches each connection through its
/// server's command loop and then its worker, so a head finished at once can reach the app before
/// the connection learns of the stop.
const STOP_SETTLES: Duration = Duration::from_millis(250);

/// A request whose head is half written before the shutdown, answered as the host's
/// `drain_pending` declares: where `Served`, the head is finished once the host has had
/// `STOP_SETTLES` to apply its stop, and the app answers 503 with `Connection: close`; where
/// `Closed`, the head is never finished and the host closes the connection with no answer, since a
/// head finished before the host processes its stop would reach the app.
///
/// A host that closes its listener when the drain begins resets a connection still in the
/// backlog. A request answered on a second connection opened after this one shows the host
/// has accepted this one, since a listener hands connections over in the order they arrived.
pub async fn http1<H: Host>(mode: Mode) {
    let host = start::<H>(mode).await;
    let mut raw = Raw::connect(&host.authority()).await;
    raw.write(format!("GET {} HTTP/1.1\r\nHost: suite\r\n", host.target("/hit")).as_bytes()).await;
    let accepted = host.send(Exchange::get("/hit")).await;
    assert_eq!(accepted.status, 200, "the request proving the first connection was accepted");
    let closing = begin_close(&host).await;
    match H::limits().drain_pending {
        DrainPending::Served => {
            tokio::time::sleep(STOP_SETTLES).await;
            raw.write(b"\r\n").await;
            let answer = raw.read_to_close("the host declares `DrainPending::Served`, so a request finished during the drain is answered").await;
            assert_eq!(
                status_of(&answer),
                Some(503),
                "the host declares `DrainPending::Served` and did not answer a request finished during the drain with 503: {:?}",
                String::from_utf8_lossy(&answer)
            );
            assert!(has_header(&answer, "connection", "close"), "the drain's 503 carries `Connection: close`");
        }
        DrainPending::Closed => {
            let answer = raw
                .read_to_close("the host declares `DrainPending::Closed` and kept a connection whose request head was still arriving open through the drain: declare `Served`")
                .await;
            assert!(
                answer.is_empty(),
                "the host declares `DrainPending::Closed` and wrote to a connection whose request head was still arriving: {:?}",
                String::from_utf8_lossy(&answer)
            );
        }
    }
    let _ = closing.await;
    host.stop().await;
}

/// A long-lived stream holds an HTTP/2 connection open through the drain, and a request sent
/// during the drain is not served: the host answers it 503 on the held connection, or refuses the
/// new connection the client opens after GOAWAY. Either happens within half the drain window; a
/// request that ends only when the host's own stop deadline cuts it was left waiting in silence.
pub async fn http2<H: Host>(mode: Mode) {
    let host = start::<H>(mode).await;
    let client = reqwest::Client::builder().http2_prior_knowledge().timeout(PATIENCE).build().expect("an HTTP/2 client");
    let served = client.get(host.url("/hit")).send().await.unwrap_or_else(|error| {
        not_a_timeout(&error, "an HTTP/2 request before the drain");
        panic!("the host does not serve HTTP/2 without TLS: declare `drain_http2` not applicable ({error})")
    });
    assert_eq!(served.status().as_u16(), 200, "an HTTP/2 request before the drain");
    let held = client.get(host.url("/endless")).send().await.expect("the stream opens");
    let closing = begin_close(&host).await;
    let sent = tokio::time::Instant::now();
    match client.get(host.url("/hit")).send().await {
        Ok(response) => assert_eq!(response.status().as_u16(), 503, "an HTTP/2 request during the drain was served"),
        Err(error) => not_a_timeout(&error, "an HTTP/2 request during the drain"),
    }
    let window = host.app.drain_timeout();
    let took = sent.elapsed();
    assert!(
        took < window / 2,
        "an HTTP/2 request during the drain ended after {took:?} of a {window:?} drain window, neither answered nor refused before then"
    );
    drop(held);
    let _ = closing.await;
    host.stop().await;
}

/// An HTTP/2 client holding a connection, a stream open on it, receives GOAWAY with `NO_ERROR`
/// once the drain begins: its next request on that connection fails as a GOAWAY the host sent.
pub async fn goaway<H: Host>(mode: Mode) {
    let host = start::<H>(mode).await;
    let (mut send, _connection) = h2_connect(&host).await;
    let request = http::Request::get(host.url("/endless")).body(()).expect("a request");
    send = tokio::time::timeout(PATIENCE, send.ready())
        .await
        .expect("the connection is ready within the patience")
        .expect("the connection is ready");
    let (response, _) = send.send_request(request, true).expect("the stream opens");
    let response = tokio::time::timeout(PATIENCE, response)
        .await
        .expect("the stream answers within the patience")
        .expect("the stream answers");
    assert_eq!(response.status().as_u16(), 200, "the held stream");
    let mut held = response.into_body();
    let first = tokio::time::timeout(PATIENCE, held.data()).await.expect("the stream's first event within the patience");
    assert!(first.is_some_and(|chunk| chunk.is_ok()), "the held stream's first event");

    let closing = begin_close(&host).await;
    let deadline = tokio::time::Instant::now() + PATIENCE;
    let error = loop {
        match send.clone().ready().await {
            Ok(_) => {
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "no GOAWAY reached the client within {PATIENCE:?} of the drain beginning"
                );
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(error) => break error,
        }
    };
    assert!(error.is_go_away() && error.is_remote(), "the connection failed otherwise than by the host's GOAWAY: {error}");
    assert_eq!(error.reason(), Some(h2::Reason::NO_ERROR), "the drain's GOAWAY reports an error: {error}");
    drop(held);
    drop(send);
    let _ = closing.await;
    host.stop().await;
}

/// An HTTP/2 connection to the host, driven on its own task until it ends.
async fn h2_connect<H: Host>(host: &Running<H>) -> (h2::client::SendRequest<Bytes>, tokio::task::JoinHandle<()>) {
    let tcp = TcpStream::connect(host.authority()).await.expect("the host accepts a connection");
    let handshake = tokio::time::timeout(PATIENCE, h2::client::handshake(tcp)).await.unwrap_or_else(|_| {
        panic!("the HTTP/2 handshake did not finish within {PATIENCE:?}, which is neither an answer nor a refusal")
    });
    let (send, connection) =
        handshake.unwrap_or_else(|error| panic!("the host does not serve HTTP/2 without TLS: declare the scenario not applicable ({error})"));
    let driving = tokio::spawn(async move {
        let _ = connection.await;
    });
    (send, driving)
}

/// Starts the app's `close` on its own task and waits for the drain to begin.
async fn begin_close<H: Host>(host: &Running<H>) -> tokio::task::JoinHandle<()> {
    let app = host.app.clone();
    let closing = tokio::spawn(async move {
        let _ = app.close(Signal::new("suite")).await;
    });
    tokio::time::timeout(PATIENCE, host.app.draining()).await.expect("the drain begins");
    closing
}

/// A response in flight when the drain begins, abandoned by the client at once, ends the host's
/// graceful stop as the host's `drain_abandoned` declares. The app's `close` waits for the host,
/// so it measures the stop: under half the drain window where `Released`, since the host drops
/// the response at its declared `disconnect` moment, and at least the whole window where
/// `Window`.
pub async fn abandoned<H: Host>(mode: Mode) {
    let host = start::<H>(mode).await;
    let window = host.app.drain_timeout();
    let mut raw = Raw::connect(&host.authority()).await;
    let request = format!("GET {} HTTP/1.1\r\nHost: suite\r\nAccept: text/event-stream\r\n\r\n", host.target("/endless"));
    raw.write(request.as_bytes()).await;
    raw.read_until(b"data: start", "the stream's first event").await.expect("the stream's first event arrives");
    let started = tokio::time::Instant::now();
    let closing = begin_close(&host).await;
    drop(raw);
    let bound = window + PATIENCE;
    tokio::time::timeout(bound, closing)
        .await
        .unwrap_or_else(|_| panic!("the app's `close` had not finished {bound:?} after the drain began"))
        .expect("the close task completes");
    let took = started.elapsed();
    match H::limits().drain_abandoned {
        DrainAbandoned::Released => assert!(
            took < window / 2,
            "the host declares `DrainAbandoned::Released` and its stop took {took:?} of a {window:?} drain window: declare `Window`"
        ),
        DrainAbandoned::Window => assert!(
            took >= window,
            "the host declares `DrainAbandoned::Window` and its stop ended after {took:?} of a {window:?} drain window: declare `Released`"
        ),
    }
    host.stop().await;
}
