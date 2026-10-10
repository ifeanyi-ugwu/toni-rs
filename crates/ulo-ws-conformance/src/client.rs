//! The suite's client: it writes each upgrade request itself over the host's stream, so a scenario
//! can send one RFC 6455 refuses and read the status, headers and body exactly as the server wrote
//! them, then speaks WebSocket through `async-tungstenite` on the same stream after a 101.

use std::sync::Arc;

use async_tungstenite::WebSocketStream;
use async_tungstenite::tungstenite::Message;
use async_tungstenite::tungstenite::handshake::client::generate_key;
use async_tungstenite::tungstenite::handshake::derive_accept_key;
use async_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use async_tungstenite::tungstenite::protocol::{CloseFrame, Role};
use futures_io::{AsyncRead, AsyncWrite};
use futures_util::{AsyncReadExt, AsyncWriteExt, StreamExt};
use serde_json::Value;
use ulo::Timer;

use crate::app::within;

/// One upgrade request, valid until a scenario spoils it.
#[derive(Clone, Debug)]
pub(crate) struct Request {
    method: String,
    path: String,
    version: &'static str,
    headers: Vec<(String, String)>,
    key: String,
}

impl Request {
    /// A valid upgrade request for `path` with a fresh key.
    pub(crate) fn upgrade(path: &str) -> Request {
        Request { method: "GET".to_owned(), path: path.to_owned(), version: "HTTP/1.1", headers: Vec::new(), key: String::new() }
            .set("Connection", "Upgrade")
            .set("Upgrade", "websocket")
            .set("Sec-WebSocket-Version", "13")
            .key(&generate_key())
    }

    pub(crate) fn key(mut self, key: &str) -> Request {
        self.key = key.to_owned();
        self.set("Sec-WebSocket-Key", key)
    }

    pub(crate) fn method(mut self, method: &str) -> Request {
        self.method = method.to_owned();
        self
    }

    pub(crate) fn version(mut self, version: &'static str) -> Request {
        self.version = version;
        self
    }

    /// One more header line, beside any of the same name.
    pub(crate) fn header(mut self, name: &str, value: &str) -> Request {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }

    /// `name` set to `value`, replacing every line of that name.
    pub(crate) fn set(self, name: &str, value: &str) -> Request {
        self.without(name).header(name, value)
    }

    pub(crate) fn without(mut self, name: &str) -> Request {
        self.headers.retain(|(line, _)| !line.eq_ignore_ascii_case(name));
        self
    }

    /// The request head as written on the wire, `host` its `Host` header.
    pub(crate) fn head(&self, host: &str) -> String {
        let mut head = format!("{} {} {}\r\nHost: {host}\r\n", self.method, self.path, self.version);
        for (name, value) in &self.headers {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        head.push_str("\r\n");
        head
    }

    pub(crate) fn path(&self) -> &str {
        &self.path
    }

    pub(crate) fn sent_key(&self) -> &str {
        &self.key
    }
}

/// The server's answer to one upgrade request: its status, its headers with lowercase names in
/// the order written, its body for a refusal, and the socket after a 101.
pub(crate) struct Answer<S> {
    pub(crate) status: u16,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) body: String,
    pub(crate) socket: Option<WebSocketStream<S>>,
}

impl<S> Answer<S> {
    /// The first value of header `name`, given in lowercase.
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(key, _)| key == name).map(|(_, value)| value.as_str())
    }
}

/// Writes `text` on `stream`.
pub(crate) async fn write<S: AsyncWrite + Unpin>(stream: &mut S, text: &str) {
    stream.write_all(text.as_bytes()).await.unwrap_or_else(|error| panic!("writing the upgrade request failed: {error}"));
    stream.flush().await.unwrap_or_else(|error| panic!("flushing the upgrade request failed: {error}"));
}

/// Reads the server's answer to an upgrade request whose key was `key`. A 101 must carry the
/// `Sec-WebSocket-Accept` RFC 6455 §4.2.2 derives from that key, and the stream continues as the
/// client's socket. A refusal's body is read by its `Content-Length`, or its chunks.
pub(crate) async fn read_answer<S: AsyncRead + AsyncWrite + Unpin>(mut stream: S, key: &str) -> Answer<S> {
    // One byte at a time, so nothing after the head, a Close frame sent right after the 101 for
    // one, is taken from the stream the socket continues on.
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        let mut byte = [0u8];
        stream.read_exact(&mut byte).await.unwrap_or_else(|error| {
            panic!("reading the upgrade response failed after {:?}: {error}", String::from_utf8_lossy(&head))
        });
        head.push(byte[0]);
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
    let mut answer = Answer { status, headers, body: String::new(), socket: None };
    if status == 101 {
        let expected = derive_accept_key(key.as_bytes());
        assert_eq!(answer.header("sec-websocket-accept"), Some(expected.as_str()), "the 101 carries the wrong `Sec-WebSocket-Accept`");
        answer.socket = Some(WebSocketStream::from_raw_socket(stream, Role::Client, None).await);
    } else if let Some(length) = answer.header("content-length").and_then(|length| length.parse::<usize>().ok()) {
        let mut body = vec![0; length];
        stream.read_exact(&mut body).await.unwrap_or_else(|error| panic!("reading the refusal's body failed: {error}"));
        answer.body = String::from_utf8_lossy(&body).into_owned();
    } else if answer.header("transfer-encoding").is_some_and(|coding| coding.eq_ignore_ascii_case("chunked")) {
        answer.body = chunked(&mut stream).await;
    }
    answer
}

/// A chunked body, read to its last chunk.
async fn chunked<S: AsyncRead + Unpin>(stream: &mut S) -> String {
    let mut body = Vec::new();
    loop {
        let mut line = Vec::new();
        while !line.ends_with(b"\r\n") {
            let mut byte = [0u8];
            stream.read_exact(&mut byte).await.unwrap_or_else(|error| panic!("reading a chunk's size failed: {error}"));
            line.push(byte[0]);
        }
        let size = String::from_utf8_lossy(&line[..line.len() - 2]).split(';').next().unwrap_or_default().trim().to_owned();
        let size = usize::from_str_radix(&size, 16).unwrap_or_else(|_| panic!("a chunk's size is malformed: {size:?}"));
        let mut chunk = vec![0; size + 2];
        stream.read_exact(&mut chunk).await.unwrap_or_else(|error| panic!("reading a chunk failed: {error}"));
        if size == 0 {
            return String::from_utf8_lossy(&body).into_owned();
        }
        body.extend_from_slice(&chunk[..size]);
    }
}

/// An open WebSocket connection, every read bounded by the app's clock.
pub(crate) struct Conn<S> {
    pub(crate) ws: WebSocketStream<S>,
    timer: Arc<dyn Timer>,
}

impl<S: AsyncRead + AsyncWrite + Unpin> Conn<S> {
    pub(crate) fn new(ws: WebSocketStream<S>, timer: Arc<dyn Timer>) -> Self {
        Conn { ws, timer }
    }

    pub(crate) async fn send(&mut self, message: Message) {
        let shown = format!("{message:?}");
        self.ws.send(message).await.unwrap_or_else(|error| panic!("sending {shown} failed: {error}"));
    }

    /// `message` as one text frame.
    pub(crate) async fn send_json(&mut self, message: &Value) {
        self.send(Message::text(message.to_string())).await;
    }

    /// The next thing the socket yields, Pings and Pongs included; `None` once it has ended.
    pub(crate) async fn next_raw(&mut self, what: &str) -> Option<Result<Message, String>> {
        within(&*self.timer, what, self.ws.next()).await.map(|next| next.map_err(|error| error.to_string()))
    }

    /// The next message that is not a Ping or a Pong.
    pub(crate) async fn next_message(&mut self) -> Message {
        let timer = Arc::clone(&self.timer);
        within(&*timer, "the next message", async {
            loop {
                match self.ws.next().await {
                    Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
                    Some(Ok(message)) => return message,
                    other => panic!("expected a message, got {other:?}"),
                }
            }
        })
        .await
    }

    /// The next data message as JSON text.
    pub(crate) async fn next_json(&mut self) -> Value {
        match self.next_message().await {
            Message::Text(text) => {
                serde_json::from_str(&text).unwrap_or_else(|error| panic!("the server sent text that is not JSON, {text:?}: {error}"))
            }
            other => panic!("expected a text message, got {other:?}"),
        }
    }

    /// Sends `message` and reads the next data message.
    pub(crate) async fn exchange(&mut self, message: Value) -> Value {
        self.send_json(&message).await;
        self.next_json().await
    }

    /// The close code and reason the server ends the connection with. A data message before the
    /// Close frame fails the scenario.
    pub(crate) async fn close_frame(&mut self) -> Option<(u16, String)> {
        match self.next_message().await {
            Message::Close(frame) => frame.map(|frame| (u16::from(frame.code), frame.reason.as_str().to_owned())),
            other => panic!("expected a Close frame, got {other:?}"),
        }
    }

    /// The close code and reason, past any data messages written before the Close frame; how many
    /// there were.
    pub(crate) async fn close_frame_after_data(&mut self) -> (Option<(u16, String)>, usize) {
        let mut data = 0;
        loop {
            match self.next_message().await {
                Message::Close(frame) => return (frame.map(|frame| (u16::from(frame.code), frame.reason.as_str().to_owned())), data),
                Message::Text(_) | Message::Binary(_) => data += 1,
                other => panic!("expected data or a Close frame, got {other:?}"),
            }
        }
    }

    /// Closes the client's side with `code` and `reason`, or answers a Close the server sent, and
    /// reads until the server ends the connection.
    pub(crate) async fn hang_up_with(mut self, code: u16, reason: &str) {
        let frame = CloseFrame { code: CloseCode::from(code), reason: reason.to_owned().into() };
        let _ = self.ws.close(Some(frame)).await;
        let timer = Arc::clone(&self.timer);
        within(&*timer, "the server's end of the connection", async { while let Some(Ok(_)) = self.ws.next().await {} }).await;
    }

    pub(crate) async fn hang_up(self) {
        self.hang_up_with(1000, "").await;
    }
}
