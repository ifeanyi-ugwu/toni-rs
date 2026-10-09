//! What the WebSocket tests share: an app serving on an ephemeral port, a client that performs the
//! handshake itself, so a test reads the 101 or the refusal exactly as the server wrote it, and a
//! record a gateway writes to and a test waits on.
//!
//! Every wait is bounded by [`WAIT`] and fails the test when it runs out: no test passes because
//! something did not arrive.

#![allow(dead_code)]

use std::future::Future;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::watch;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::client::generate_key;
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, Role};
use ulo::{App, AppHandle, Module, Runtime, Server, Signal};

pub type Socket = WebSocketStream<TcpStream>;

/// The longest any one wait in these tests lasts before it fails the test.
pub const WAIT: Duration = Duration::from_secs(5);

/// `fut`'s output, or a failure naming `what` once [`WAIT`] has passed.
pub async fn within<F: Future>(what: &str, fut: F) -> F::Output {
    match tokio::time::timeout(WAIT, fut).await {
        Ok(output) => output,
        Err(_) => panic!("{what} did not happen within {WAIT:?}"),
    }
}

/// An app listening on one ephemeral port, serving until the test stops it.
pub struct Running {
    pub addr: SocketAddr,
    pub handle: AppHandle,
    serving: tokio::task::JoinHandle<()>,
}

impl Running {
    /// `root` wired, connected and bound to `server`, which must listen on one address.
    pub async fn start(root: impl Module, server: impl Server) -> Running {
        Running::start_on(root, server, ulo_tokio::Tokio::current()).await
    }

    /// [`start`](Self::start) with `runtime` as the app's runtime.
    pub async fn start_on(root: impl Module, server: impl Server, runtime: impl Runtime) -> Running {
        let app = App::builder(root)
            .runtime(runtime)
            .drain_timeout(Duration::from_secs(4))
            .wire()
            .unwrap_or_else(|error| panic!("the app did not wire: {error}"))
            .connect()
            .await
            .unwrap_or_else(|error| panic!("the app did not connect: {error}"))
            .bind(server)
            .listen()
            .await
            .unwrap_or_else(|error| panic!("the app did not listen: {error}"));
        let addr = match app.addresses().as_slice() {
            [bound] => bound.addr,
            other => panic!("expected one bound address, got {other:?}"),
        };
        let handle = app.handle();
        let serving = tokio::spawn(async move {
            let _ = app.serve(std::future::pending::<Signal>()).await;
        });
        Running { addr, handle, serving }
    }

    /// An upgrade to `path` that the server answered 101, as a client socket.
    pub async fn connect(&self, path: &str, headers: &[(&str, &str)]) -> Socket {
        let upgrade = upgrade(self.addr, path, headers).await;
        match upgrade.socket {
            Some(socket) => socket,
            None => panic!("expected the upgrade to {path} to switch protocols, got {} {:?}", upgrade.status, upgrade.body),
        }
    }

    /// Closes the app, waiting for its drain. A client still connected is sent 1001 and waited
    /// for until it answers or the drain times out, so a test hangs up its clients first.
    pub async fn stop(self) {
        within("the app's close", async {
            let _ = self.handle.close(Signal::new("test")).await;
            let _ = self.serving.await;
        })
        .await;
    }
}

/// The server's answer to one upgrade request: its status, its headers with lowercase names, its
/// body for a refusal, and the socket after a 101.
pub struct Upgrade {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
    pub socket: Option<Socket>,
}

impl Upgrade {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(key, _)| key == name).map(|(_, value)| value.as_str())
    }
}

/// Sends a WebSocket upgrade request for `path` with `headers` and reads the answer as written.
///
/// The client checks the 101's `Sec-WebSocket-Accept` and nothing else: tungstenite's own client
/// fails a handshake whose 101 echoes no subprotocol when one was offered, which RFC 6455 §4.2.2
/// allows the server to do, and a test needs to see that 101.
pub async fn upgrade(addr: SocketAddr, path: &str, headers: &[(&str, &str)]) -> Upgrade {
    within(&format!("the upgrade to {path}"), async {
        let mut stream = TcpStream::connect(addr).await.unwrap_or_else(|error| panic!("connecting to {addr} failed: {error}"));
        let key = generate_key();
        let mut request = format!(
            "GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\
             Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: {key}\r\n"
        );
        for (name, value) in headers {
            request.push_str(&format!("{name}: {value}\r\n"));
        }
        request.push_str("\r\n");
        stream.write_all(request.as_bytes()).await.unwrap_or_else(|error| panic!("writing the upgrade request failed: {error}"));

        // Read one byte at a time, so nothing after the head, a Close frame sent right after the
        // 101 for one, is taken from the socket the client continues on.
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            let byte = stream.read_u8().await.unwrap_or_else(|error| panic!("reading the upgrade response failed: {error}"));
            head.push(byte);
        }
        let head = String::from_utf8(head).unwrap_or_else(|error| panic!("the upgrade response head is not UTF-8: {error}"));
        let mut lines = head.split("\r\n");
        let status_line = lines.next().unwrap_or_default();
        let status: u16 = status_line
            .split(' ')
            .nth(1)
            .and_then(|code| code.parse().ok())
            .unwrap_or_else(|| panic!("the upgrade response's status line is malformed: {status_line:?}"));
        let headers: Vec<(String, String)> = lines
            .filter(|line| !line.is_empty())
            .filter_map(|line| line.split_once(':'))
            .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
            .collect();
        let mut upgrade = Upgrade { status, headers, body: String::new(), socket: None };
        if status == 101 {
            let expected = derive_accept_key(key.as_bytes());
            assert_eq!(upgrade.header("sec-websocket-accept"), Some(expected.as_str()), "the 101 carries the wrong accept key");
            upgrade.socket = Some(WebSocketStream::from_raw_socket(stream, Role::Client, None).await);
        } else if let Some(length) = upgrade.header("content-length").and_then(|length| length.parse::<usize>().ok()) {
            let mut body = vec![0; length];
            stream.read_exact(&mut body).await.unwrap_or_else(|error| panic!("reading the refusal's body failed: {error}"));
            upgrade.body = String::from_utf8_lossy(&body).into_owned();
        }
        upgrade
    })
    .await
}

/// Sends `message` as one text frame.
pub async fn send_json(socket: &mut Socket, message: &Value) {
    socket.send(Message::text(message.to_string())).await.unwrap_or_else(|error| panic!("sending {message} failed: {error}"));
}

/// The next message that is not a Ping or a Pong.
pub async fn next_message(socket: &mut Socket) -> Message {
    within("the next message", async {
        loop {
            match socket.next().await {
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
                Some(Ok(message)) => return message,
                other => panic!("expected a message, got {other:?}"),
            }
        }
    })
    .await
}

/// The next data message as JSON text.
pub async fn next_json(socket: &mut Socket) -> Value {
    match next_message(socket).await {
        Message::Text(text) => {
            serde_json::from_str(&text).unwrap_or_else(|error| panic!("the server sent non-JSON text {text:?}: {error}"))
        }
        other => panic!("expected a text message, got {other:?}"),
    }
}

/// The close code and reason the server ends `socket` with. A data message before the Close frame
/// fails the test.
pub async fn close_frame(socket: &mut Socket) -> Option<(u16, String)> {
    match next_message(socket).await {
        Message::Close(frame) => frame.map(|frame| (u16::from(frame.code), frame.reason.as_str().to_owned())),
        other => panic!("expected a Close frame, got {other:?}"),
    }
}

/// Closes the client's side with `code` and `reason`, or answers a Close the server sent, and
/// reads until the server ends the connection.
pub async fn hang_up_with(mut socket: Socket, code: u16, reason: &str) {
    let frame = CloseFrame { code: CloseCode::from(code), reason: reason.to_owned().into() };
    let _ = socket.close(Some(frame)).await;
    within("the server's end of the connection", async { while let Some(Ok(_)) = socket.next().await {} }).await;
}

pub async fn hang_up(socket: Socket) {
    hang_up_with(socket, 1000, "").await;
}

/// Items a gateway writes while a test runs, which the test reads back or waits for.
pub struct Record<T> {
    items: Arc<Mutex<Vec<T>>>,
    count: Arc<watch::Sender<usize>>,
}

impl<T> Clone for Record<T> {
    fn clone(&self) -> Self {
        Record { items: Arc::clone(&self.items), count: Arc::clone(&self.count) }
    }
}

impl<T: Clone> Record<T> {
    pub fn new() -> Self {
        Record { items: Arc::new(Mutex::new(Vec::new())), count: Arc::new(watch::channel(0).0) }
    }

    pub fn push(&self, item: T) {
        let mut items = self.items.lock().unwrap_or_else(PoisonError::into_inner);
        items.push(item);
        let len = items.len();
        drop(items);
        self.count.send_replace(len);
    }

    pub fn snapshot(&self) -> Vec<T> {
        self.items.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Every item once at least `n` have been written.
    pub async fn at_least(&self, n: usize, what: &str) -> Vec<T> {
        let mut count = self.count.subscribe();
        within(what, count.wait_for(|len| *len >= n)).await.unwrap_or_else(|_| panic!("the record of {what} closed"));
        self.snapshot()
    }
}
