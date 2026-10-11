//! Scenarios: drain. A request still arriving once the app has stopped admitting executions is
//! answered 503, and the connection closes after it, by `Connection: close` on HTTP/1.1, and on
//! HTTP/2 by GOAWAY or at the host's stop deadline as its `drain_http2` declares.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::Empty;
use tokio_util::compat::FuturesAsyncReadCompatExt;
use ulo::{Signal, TaskHandle};
use ulo_http::embed::{DrainAbandoned, DrainHttp2, DrainPending};
use ulo_hyper_serve::{FuturesIo, RuntimeExecutor};

use crate::wire::{Failure, PATIENCE, Running, has_header, not_a_timeout, start, status_of, within};
use crate::{Host, Mode};

/// A request whose head is half written before the shutdown, answered as the host's
/// `drain_pending` declares: where `Served`, the head is finished once the drain has begun, and
/// the app answers 503 with `Connection: close`; where `Closed`, the head is never finished and the
/// host closes the connection with no answer, since a head finished before the host processes its
/// stop would reach the app.
///
/// A host that closes its listener when the drain begins resets a connection still in the
/// backlog, and hyper's graceful shutdown closes as idle a connection it has read nothing from, so
/// the drain begins only once the host's own count of connections read from includes this one,
/// and no accept order is assumed. A host whose server cannot count fails here, and declares the
/// scenario not applicable.
pub async fn http1<H: Host>(mode: Mode) {
    let host = start::<H>(mode).await;
    let Some(counted) = host.host.connections_read() else {
        panic!(
            "the host reports no count of the connections it read from, so nothing shows a connection carrying half a request was \
             in progress before the drain: declare `drain_http1` not applicable with the reason it cannot count"
        )
    };
    let mut raw = host.raw().await;
    raw.write(format!("GET {} HTTP/1.1\r\nHost: suite\r\n", host.target("/hit")).as_bytes()).await;
    host.read_past(counted, "the host's count including the connection carrying half a request").await;
    let closing = begin_close(&host).await;
    match H::limits().drain_pending {
        DrainPending::Served => {
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
/// during the drain is not served. Where the host declares `DrainHttp2::GoAway`, it answers the
/// request 503 on the held connection, or refuses the new connection the client opens after
/// GOAWAY; where `Reset`, no GOAWAY tells the client to leave the held connection, so the request
/// travels on it and is answered 503. Either happens within half the drain window; a request that
/// ends only when the host's own stop deadline cuts it was left waiting in silence.
pub async fn http2<H: Host>(mode: Mode) {
    let host = start::<H>(mode).await;
    let client = H2c::open(&host).await;
    let served = client.get(&host, "/hit").await.unwrap_or_else(|error| {
        not_a_timeout(&error, "an HTTP/2 request before the drain");
        panic!("the host does not serve HTTP/2 without TLS: declare `drain_http2` not applicable ({error})")
    });
    assert_eq!(served.status().as_u16(), 200, "an HTTP/2 request before the drain");
    let held = client.get(&host, "/endless").await.unwrap_or_else(|error| panic!("the stream opens: {error}"));
    let closing = begin_close(&host).await;
    let sent = host.timer().now();
    match (client.get(&host, "/hit").await, H::limits().drain_http2) {
        (Ok(response), _) => assert_eq!(response.status().as_u16(), 503, "an HTTP/2 request during the drain was served"),
        (Err(error), DrainHttp2::GoAway) => not_a_timeout(&error, "an HTTP/2 request during the drain"),
        (Err(error), DrainHttp2::Reset) => {
            not_a_timeout(&error, "an HTTP/2 request during the drain");
            panic!(
                "the host declares `DrainHttp2::Reset`, so the held connection stays open through the drain and carries the \
                 request to the app's 503, but the request failed: {error}"
            )
        }
    }
    let window = host.app.drain_timeout();
    let took = host.timer().now().saturating_duration_since(sent);
    assert!(
        took < window / 2,
        "an HTTP/2 request during the drain ended after {took:?} of a {window:?} drain window, neither answered nor refused before then"
    );
    drop(held);
    drop(client);
    let _ = closing.await;
    host.stop().await;
}

/// One HTTP/2 connection with prior knowledge, through hyper's client, its tasks on the app's
/// runtime: what a client holding one connection through the drain sees.
struct H2c {
    sender: hyper::client::conn::http2::SendRequest<Empty<Bytes>>,
}

impl H2c {
    async fn open<H: Host>(host: &Running<H>) -> H2c {
        let stream = host.connection().await;
        let handshake = hyper::client::conn::http2::handshake(RuntimeExecutor::new(Arc::clone(&host.runtime)), FuturesIo::new(stream));
        let (sender, connection) = within(host.timer(), PATIENCE, handshake)
            .await
            .unwrap_or_else(|| panic!("the HTTP/2 handshake did not finish within {PATIENCE:?}, which is neither an answer nor a refusal"))
            .unwrap_or_else(|error| panic!("the host does not serve HTTP/2 without TLS: declare the scenario not applicable ({error})"));
        drop(host.runtime.spawn(Box::pin(async move {
            let _ = connection.await;
        })));
        H2c { sender }
    }

    /// `GET path`, its response head within [`PATIENCE`]; the body is left to the caller.
    async fn get<H: Host>(&self, host: &Running<H>, path: &str) -> Result<http::Response<hyper::body::Incoming>, Failure> {
        let request = http::Request::get(host.url(path)).body(Empty::new()).expect("a request");
        let mut sender = self.sender.clone();
        let sent = async move {
            sender.ready().await?;
            sender.send_request(request).await
        };
        match within(host.timer(), PATIENCE, sent).await {
            None => Err(Failure::TimedOut),
            Some(Err(error)) => Err(Failure::Failed(error.to_string())),
            Some(Ok(response)) => Ok(response),
        }
    }
}

/// An HTTP/2 client holds a connection, a stream open on it, through the drain, and the
/// connection ends as the host's `drain_http2` declares. Where `GoAway`, the client receives
/// GOAWAY with `NO_ERROR` within the patience: its next request on that connection fails as a
/// GOAWAY the host sent. Where `Reset`, the connection takes requests until the host's stop
/// deadline, no sooner than the drain window, and then fails with no GOAWAY received.
pub async fn goaway<H: Host>(mode: Mode) {
    let host = start::<H>(mode).await;
    let mut send = h2_connect(&host).await;
    let request = http::Request::get(host.url("/endless")).body(()).expect("a request");
    send = within(host.timer(), PATIENCE, send.ready())
        .await
        .expect("the connection is ready within the patience")
        .expect("the connection is ready");
    let (response, _) = send.send_request(request, true).expect("the stream opens");
    let response = within(host.timer(), PATIENCE, response)
        .await
        .expect("the stream answers within the patience")
        .expect("the stream answers");
    assert_eq!(response.status().as_u16(), 200, "the held stream");
    let mut held = response.into_body();
    let first = within(host.timer(), PATIENCE, held.data()).await.expect("the stream's first event within the patience");
    assert!(first.is_some_and(|chunk| chunk.is_ok()), "the held stream's first event");

    let window = host.app.drain_timeout();
    let declared = H::limits().drain_http2;
    let patience = match declared {
        DrainHttp2::GoAway => PATIENCE,
        DrainHttp2::Reset => window + PATIENCE,
    };
    let started = host.timer().now();
    let closing = begin_close(&host).await;
    let error = loop {
        match send.clone().ready().await {
            Ok(_) => {
                assert!(
                    host.timer().now().saturating_duration_since(started) < patience,
                    "the host declares `{declared:?}` and its connection was neither sent GOAWAY nor ended within {patience:?} of the drain beginning"
                );
                host.timer().sleep(Duration::from_millis(20)).await;
            }
            Err(error) => break error,
        }
    };
    let took = host.timer().now().saturating_duration_since(started);
    match declared {
        DrainHttp2::GoAway => {
            assert!(
                error.is_go_away() && error.is_remote(),
                "the host declares `DrainHttp2::GoAway` and the connection failed otherwise than by its GOAWAY: {error}"
            );
            assert_eq!(error.reason(), Some(h2::Reason::NO_ERROR), "the drain's GOAWAY reports an error: {error}");
        }
        DrainHttp2::Reset => {
            assert!(
                !(error.is_go_away() && error.is_remote()),
                "the host declares `DrainHttp2::Reset` and sent GOAWAY after {took:?}: declare `GoAway`"
            );
            assert!(
                took >= window,
                "the host declares `DrainHttp2::Reset` and the connection ended after {took:?} of a {window:?} drain window, before its stop deadline: {error}"
            );
        }
    }
    drop(held);
    drop(send);
    let _ = closing.await;
    host.stop().await;
}

/// An HTTP/2 connection to the host through `h2` itself, which reports the GOAWAY frame, over the
/// harness's stream seen through tokio's I/O traits, which are what `h2` reads; driven on a task of
/// the app's runtime until it ends.
async fn h2_connect<H: Host>(host: &Running<H>) -> h2::client::SendRequest<Bytes> {
    let stream = host.connection().await;
    let handshake = within(host.timer(), PATIENCE, h2::client::handshake(stream.compat())).await.unwrap_or_else(|| {
        panic!("the HTTP/2 handshake did not finish within {PATIENCE:?}, which is neither an answer nor a refusal")
    });
    let (send, connection) =
        handshake.unwrap_or_else(|error| panic!("the host does not serve HTTP/2 without TLS: declare the scenario not applicable ({error})"));
    drop(host.runtime.spawn(Box::pin(async move {
        let _ = connection.await;
    })));
    send
}

/// Starts the app's `close` on a task of the app's runtime and waits for the drain to begin.
async fn begin_close<H: Host>(host: &Running<H>) -> TaskHandle {
    let app = host.app.clone();
    let closing = host.runtime.spawn(Box::pin(async move {
        let _ = app.close(Signal::new("suite")).await;
    }));
    within(host.timer(), PATIENCE, host.app.draining()).await.expect("the drain begins");
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
    let mut raw = host.raw().await;
    let request = format!("GET {} HTTP/1.1\r\nHost: suite\r\nAccept: text/event-stream\r\n\r\n", host.target("/endless"));
    raw.write(request.as_bytes()).await;
    raw.read_until(b"data: start", "the stream's first event").await.expect("the stream's first event arrives");
    let started = host.timer().now();
    let closing = begin_close(&host).await;
    drop(raw);
    let bound = window + PATIENCE;
    let ended = within(host.timer(), bound, closing)
        .await
        .unwrap_or_else(|| panic!("the app's `close` had not finished {bound:?} after the drain began"));
    assert!(ended.is_finished(), "the close task completes: it ended {ended:?}");
    let took = host.timer().now().saturating_duration_since(started);
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
