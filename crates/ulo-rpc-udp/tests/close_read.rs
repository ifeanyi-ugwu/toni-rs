//! A server's `close` answers the requests its socket already holds. The receive task first waits
//! on an empty socket, as an idle server's does; the datagrams are then sent on a current-thread
//! runtime while the test's task keeps it from running, so each is in the socket's buffer, read by
//! no one, when `close` stops the receive task. Only `close`'s own read can answer them.

use std::net::UdpSocket as StdUdpSocket;
use std::time::Duration;

use bytes::Bytes;
use ulo::{App, Module, ModuleDef, ModuleIdentity, Signal};
use ulo_rpc::{CallHeaders, Codec, Data, Frame, Link, Pattern};
use ulo_rpc_udp::Udp;
use ulo_transport::ErrorKind;

/// Requests left in the socket at close.
const LEFT: u64 = 5;

/// How long the caller waits for each refusal.
const PATIENCE: Duration = Duration::from_secs(5);

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        let _ = m;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn close_answers_the_requests_its_socket_already_holds() {
    let app = App::builder(Root)
        .timer(ulo_tokio::Timer)
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects");
    let mut udp = Udp::new("127.0.0.1:0");
    udp.prepare(&app.handle()).await.expect("the link prepares");
    let inbound = udp.listen(&[Pattern::from("close_read.add")]).await.expect("the link listens");
    let addr = udp.bound().first().map(|bound| bound.addr).expect("the link's address");
    // The receive task reads the empty socket and waits.
    tokio::task::yield_now().await;

    let caller = StdUdpSocket::bind("127.0.0.1:0").expect("a loopback port");
    caller.connect(addr).expect("the caller aims at the server");
    caller.set_read_timeout(Some(PATIENCE)).expect("a read timeout");
    // Blocking sends with no await between them: the receive task cannot run before `close`.
    for id in 1..=LEFT {
        let request = Frame::Req {
            id,
            pattern: "close_read.add".to_owned(),
            headers: CallHeaders::new(),
            data: Data::new(Bytes::from_static(b"1")),
        };
        let bytes = Codec::Json.encode_frame(&request).expect("the request encodes");
        caller.send(&bytes).expect("the request is sent");
    }

    udp.close().await.expect("the link closes");
    drop(inbound);

    let mut refused = Vec::new();
    let mut datagram = vec![0u8; 65_536];
    for _ in 0..LEFT {
        let len = match caller.recv(&mut datagram) {
            Ok(len) => len,
            Err(error) => panic!("{} of {LEFT} requests left in the socket at close were answered; the next read failed: {error}", refused.len()),
        };
        match Codec::Json.decode_frame(&datagram[..len]) {
            Ok(Frame::Err { id, error }) if error.kind == ErrorKind::Unavailable => refused.push(id),
            other => panic!("a request left in the socket at close was not refused `unavailable`, got: {other:?}"),
        }
    }
    refused.sort_unstable();
    assert_eq!(refused, (1..=LEFT).collect::<Vec<_>>(), "the refusals do not answer each request once");
    let _ = app.close(Signal::new("close read test")).await;
}
