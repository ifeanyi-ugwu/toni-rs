//! `on_stream_end` reports the reply the dispatcher writes: a stream an interceptor puts in place of
//! the one it discarded reports its own end. A written stream reporting `Completed` and a discarded
//! one reporting nothing are the WebSocket conformance suite's `stream_end` scenarios
//! (`tests/conformance.rs`).
//!
//! Each handler registers a callback that sends the outcome on a channel the test holds the other
//! end of. The callback owns the channel's only sender, so the channel closes when the message's
//! execution ends: a test reads `None` for a stream that reported nothing, with no wait for a
//! report that might still come.

mod support;

use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;

use futures_util::{Stream, stream};
use serde_json::json;
use tokio::sync::mpsc;
use ulo::{BoxError, ExecutionRef, Interceptor, Module, ModuleDef, ModuleIdentity, Next, StreamOutcome, injectable, routes};
use ulo_transport::{Classify, ErrorKind};
use ulo_ws::{Frame, Reply, Ws, WsCx, WsModule};

use support::{Running, hang_up, next_json, send_json};

const REPLACED: &str = "replaced";

/// How long a test waits for the message's execution to end; it ends once the reply is written.
const PATIENCE: Duration = Duration::from_secs(5);

#[derive(Debug)]
struct Late;

impl fmt::Display for Late {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("late")
    }
}

impl Error for Late {}

impl Classify for Late {
    fn classify(&self) -> ErrorKind {
        ErrorKind::Internal
    }
}

/// The sender for each event's message, taken by the handler that reports for it.
fn outcomes() -> MutexGuard<'static, HashMap<&'static str, mpsc::UnboundedSender<StreamOutcome>>> {
    static OUTCOMES: OnceLock<Mutex<HashMap<&'static str, mpsc::UnboundedSender<StreamOutcome>>>> = OnceLock::new();
    OUTCOMES.get_or_init(Mutex::default).lock().unwrap_or_else(PoisonError::into_inner)
}

/// Registers the callback that reports `event`'s outcome, owning the channel's one sender.
fn report_for(event: &'static str, exec: &ExecutionRef) {
    let sender = outcomes().remove(event).expect("each event is sent once");
    exec.on_stream_end(move |outcome| {
        let _ = sender.send(outcome);
    });
}

fn ticks() -> impl Stream<Item = Result<u32, Late>> {
    stream::iter([Ok(1), Ok(2)])
}

/// Runs the message, drops the stream it answers and answers a stream of its own.
struct AnswerStream;

impl Interceptor<Ws> for AnswerStream {
    async fn intercept(&self, _cx: &WsCx, next: Next<'_, Ws>) -> Result<Reply, BoxError> {
        drop(next.run().await?);
        Ok(Reply::Many(Box::pin(stream::iter([Ok(Frame::text("1")), Ok(Frame::text("2"))]))))
    }
}

#[injectable]
struct Streams;

#[routes]
#[ulo_ws::gateway(path = "/streams", port = own)]
impl Streams {
    #[ulo_ws::message("replaced")]
    #[interceptors(value = AnswerStream)]
    fn replaced(&self, exec: ExecutionRef) -> impl Stream<Item = Result<u32, Late>> {
        report_for(REPLACED, &exec);
        ticks()
    }
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(WsModule::for_root());
        m.controller::<Streams>();
    }
}

/// Sends `event` with id `"m"` and reads `frames` frames of its reply; answers them and what its
/// execution reported once it ended, `None` for nothing.
async fn call(event: &'static str, frames: usize) -> (Vec<serde_json::Value>, Option<StreamOutcome>) {
    let (sender, mut reported) = mpsc::unbounded_channel();
    outcomes().insert(event, sender);
    let app = Running::start(Root, ulo_ws_hyper::Server::new("127.0.0.1:0")).await;
    let mut socket = app.connect("/streams", &[]).await;
    send_json(&mut socket, &json!({ "event": event, "id": "m" })).await;
    let mut written = Vec::new();
    for _ in 0..frames {
        written.push(next_json(&mut socket).await);
    }
    let ended = async {
        let first = reported.recv().await;
        if first.is_some() {
            assert_eq!(reported.recv().await, None, "{event}: the execution reported its stream's end twice");
        }
        first
    };
    let outcome = tokio::time::timeout(PATIENCE, ended).await.unwrap_or_else(|_| panic!("{event}: the execution did not end within {PATIENCE:?}"));
    hang_up(socket).await;
    app.stop().await;
    (written, outcome)
}

#[tokio::test]
async fn the_stream_replacing_a_discarded_one_reports_its_own_end() {
    let (frames, outcome) = call(REPLACED, 3).await;
    let expected = vec![json!({ "id": "m", "data": 1 }), json!({ "id": "m", "data": 2 }), json!({ "id": "m", "complete": true })];
    assert_eq!(frames, expected, "the interceptor's stream was not the reply");
    assert_eq!(outcome, Some(StreamOutcome::Completed), "the reply's own end was not what the execution reported");
}
