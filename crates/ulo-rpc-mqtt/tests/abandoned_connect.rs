//! F369: a `connect` or a `listen` dropped while the broker has not yet answered takes its event
//! loop with it. The broker here is a socket that reads the CONNECT, waits for the drop, then
//! answers CONNACK: a loop left running would go on to SUBSCRIBE, and an ended one has closed the
//! connection.

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use ulo::BoxError;
use ulo_rpc::{Link, Pattern};
use ulo_rpc_mqtt::Mqtt;

/// How long the broker waits for each step of the client.
const PATIENCE: Duration = Duration::from_secs(5);

/// An MQTT 5 CONNACK: no session present, reason code success, no properties.
const CONNACK: [u8; 5] = [0x20, 0x03, 0x00, 0x00, 0x00];

const CONNECT: u8 = 1;
const SUBSCRIBE: u8 = 8;

/// Accepts the link's connection and reads its first packet, which is a CONNECT.
async fn handshake_begun(listener: &TcpListener) -> TcpStream {
    let (mut stream, _) = tokio::time::timeout(PATIENCE, listener.accept())
        .await
        .expect("the link did not connect within the patience")
        .expect("the broker's socket accepts");
    let mut buffer = [0u8; 512];
    let read = tokio::time::timeout(PATIENCE, stream.read(&mut buffer))
        .await
        .expect("the link sent nothing within the patience")
        .expect("the broker's socket reads");
    assert!(read > 0 && buffer[0] >> 4 == CONNECT, "the link's first packet is not a CONNECT: {:02x?}", &buffer[..read]);
    stream
}

/// Answers the CONNECT once the link's future has been dropped, then requires the connection to
/// end with no SUBSCRIBE on it.
async fn ended_unanswered(mut stream: TcpStream, what: &str) {
    // A closed connection may refuse the write; what is read afterwards decides.
    let _ = stream.write_all(&CONNACK).await;
    let mut buffer = [0u8; 512];
    loop {
        match tokio::time::timeout(PATIENCE, stream.read(&mut buffer)).await {
            Ok(Ok(0) | Err(_)) => return,
            Ok(Ok(read)) => {
                let kind = buffer[0] >> 4;
                assert_ne!(kind, SUBSCRIBE, "the event loop of a dropped `{what}` went on to subscribe: {:02x?}", &buffer[..read]);
            }
            Err(_) => panic!("the connection of a dropped `{what}` was still open {PATIENCE:?} after the broker answered"),
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_connect_dropped_before_the_broker_answers_ends_its_event_loop() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("the broker's socket binds");
    let link = Mqtt::url(format!("mqtt://{}", listener.local_addr().expect("the broker's socket has an address")));
    let connecting = tokio::spawn(async move { link.connect().await.map(drop) });
    let stream = handshake_begun(&listener).await;
    connecting.abort();
    let dropped: Result<Result<(), BoxError>, _> = connecting.await;
    assert!(dropped.is_err_and(|join| join.is_cancelled()), "the connect finished before the broker answered");
    ended_unanswered(stream, "connect").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_listen_dropped_before_the_broker_answers_ends_its_event_loop() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("the broker's socket binds");
    let link = Mqtt::url(format!("mqtt://{}", listener.local_addr().expect("the broker's socket has an address"))).group("abandoned");
    let listening = tokio::spawn(async move { link.listen(&[Pattern::from("abandoned.call")]).await.map(drop) });
    let stream = handshake_begun(&listener).await;
    listening.abort();
    let dropped: Result<Result<(), BoxError>, _> = listening.await;
    assert!(dropped.is_err_and(|join| join.is_cancelled()), "the listen finished before the broker answered");
    ended_unanswered(stream, "listen").await;
}
