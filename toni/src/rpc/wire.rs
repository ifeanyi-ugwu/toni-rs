//! Reply framing shared by every RPC transport.
//!
//! One reply convention crosses all seven transports: a handler's answer is
//! wrapped in `{"response":…}`, and a dispatch failure travels as
//! `{"err":{"message","status"}}`. [`RpcError::AppError`] renders through
//! [`RpcError::to_data`] into the canonical error envelope and rides the
//! `response` lane — the wire-err lane carries only
//! `PatternNotFound`/`Forbidden`/`Internal`.
//!
//! Byte-oriented transports (the brokers) send a frame's bytes as-is and pass
//! [`RpcData::Binary`] through raw — [`ResponseFrame::into_bytes`]. The
//! line-oriented transports (tcp, udp) speak JSON only, splice the correlation
//! `"id"` into the frame object, and degrade `Binary` to a `null` response —
//! [`ResponseFrame::into_json_value`].
//!
//! A streaming reply (ADR-0032) is item frames — `{"stream":…}`, or
//! `{"stream_b64":…}` for `Binary` — closed by `{"end": true}`, with
//! `{"end": true, "err": {…}}` as the error end. [`drive_reply_stream`] drains
//! a handler's stream through a transport-supplied sender; [`Inflight`] keys
//! the in-flight calls a `{"cancel": true}` notice can abort.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use futures::StreamExt;
use futures::stream::BoxStream;
use parking_lot::Mutex;
use serde_json::json;

use super::{RpcClientError, RpcData, RpcError, RpcHandlerOutput, RpcHandlerResult};

/// One framed reply, before a transport commits to its carrier form.
#[derive(Debug)]
pub enum ResponseFrame {
    /// A JSON envelope object: `{"response":…}` or `{"err":{…}}`.
    Json(serde_json::Value),
    /// A `Binary` reply passed through untouched. A client reads it back as
    /// `Binary` because it is not a recognized envelope.
    Raw(Vec<u8>),
}

impl ResponseFrame {
    /// The frame as carrier bytes — the broker form.
    pub fn into_bytes(self) -> Vec<u8> {
        match self {
            Self::Json(v) => v.to_string().into_bytes(),
            Self::Raw(b) => b,
        }
    }

    /// The frame as a JSON object for transports that cannot carry raw bytes:
    /// a `Raw` frame degrades to `{"response": null}`.
    pub fn into_json_value(self) -> serde_json::Value {
        match self {
            Self::Json(v) => v,
            Self::Raw(_) => json!({ "response": null }),
        }
    }
}

/// Frame a handler outcome into its reply.
pub fn frame_response(outcome: RpcHandlerResult) -> ResponseFrame {
    match outcome {
        Ok(RpcHandlerOutput::Single(RpcData::Binary(b))) => ResponseFrame::Raw(b),
        Ok(RpcHandlerOutput::Single(RpcData::Json(v))) => {
            ResponseFrame::Json(json!({ "response": v }))
        }
        Ok(RpcHandlerOutput::Single(RpcData::Text(s))) => {
            ResponseFrame::Json(json!({ "response": s }))
        }
        // #[event_pattern] handler but the caller expects a reply — an ack
        // closes the pending request instead of timing it out.
        Ok(RpcHandlerOutput::Empty) => ResponseFrame::Json(json!({ "response": null })),
        // A transport without the stream grammar refuses honestly. Dropping
        // the scoped stream fires the execution's cancellation token, so the
        // producer feeding it stops.
        Ok(RpcHandlerOutput::Stream(stream)) => {
            drop(stream);
            ResponseFrame::Json(json!({
                "err": {
                    "message": "streaming reply not supported on this transport yet",
                    "status": "unsupported"
                }
            }))
        }
        Err(RpcError::AppError(arc)) => match RpcError::AppError(arc).to_data() {
            RpcData::Binary(b) => ResponseFrame::Raw(b),
            RpcData::Json(v) => ResponseFrame::Json(json!({ "response": v })),
            RpcData::Text(s) => ResponseFrame::Json(json!({ "response": s })),
        },
        Err(e) => ResponseFrame::Json(json!({
            "err": { "message": e.to_string(), "status": error_status(&e) }
        })),
    }
}

/// The reply for a panicked handler. The panic is logged at the call site; the
/// caller sees a generic internal error.
pub fn frame_panic() -> ResponseFrame {
    ResponseFrame::Json(json!({
        "err": { "message": "internal server error", "status": "error" }
    }))
}

/// Parse a reply back into an [`RpcData`], unwrapping the
/// `{"response"}` / `{"err"}` envelope. Falls back to raw `Binary` when the
/// payload is not a recognized envelope.
pub fn parse_response(bytes: &[u8]) -> Result<RpcData, RpcClientError> {
    match serde_json::from_slice::<serde_json::Value>(bytes) {
        Ok(v) => {
            if let Some(response) = v.get("response") {
                Ok(RpcData::json(response.clone()))
            } else if let Some(err) = v.get("err") {
                let message = err
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("unknown error")
                    .to_string();
                let status = err
                    .get("status")
                    .and_then(|s| s.as_str())
                    .unwrap_or("error")
                    .to_string();
                Err(RpcClientError::Remote { message, status })
            } else {
                Ok(RpcData::json(v))
            }
        }
        Err(_) => Ok(RpcData::Binary(bytes.to_vec())),
    }
}

fn error_status(e: &RpcError) -> &'static str {
    match e {
        RpcError::PatternNotFound(_) => "not_found",
        RpcError::Forbidden(_) => "forbidden",
        RpcError::Internal(_) => "error",
        RpcError::AppError(_) => unreachable!(
            "RpcError::AppError is framed into the Ok+envelope branch before \
             reaching wire-Err framing"
        ),
    }
}

/// One item frame of a streaming reply. `Binary` travels base64 under its own
/// key: every stream frame must be JSON to be distinguishable from the end
/// marker, and a distinct key cannot collide with user JSON.
pub fn frame_stream_item(data: &RpcData) -> serde_json::Value {
    match data {
        RpcData::Json(v) => json!({ "stream": v }),
        RpcData::Text(s) => json!({ "stream": s }),
        RpcData::Binary(b) => json!({ "stream_b64": BASE64.encode(b) }),
    }
}

/// The clean end of a streaming reply.
pub fn frame_stream_end() -> serde_json::Value {
    json!({ "end": true })
}

/// The closing frames for an `Err` item — the two-lane rule per item.
/// `AppError` becomes a final data frame carrying the canonical envelope plus
/// a clean end, so the caller sees data; the framework variants become an
/// error end the caller's stream yields as its failure. `to_data` runs under
/// panic recovery: a panicking user `Error` impl degrades to the internal
/// envelope.
pub fn frame_stream_error(e: &RpcError) -> Vec<serde_json::Value> {
    match e {
        RpcError::AppError(_) => {
            let data = crate::panic_recovery::catch_sync(
                crate::errors::PipelineSegment::ResponseRendering,
                || e.to_data(),
            )
            .unwrap_or_else(|_| RpcData::text("Internal Server Error"));
            vec![frame_stream_item(&data), frame_stream_end()]
        }
        other => vec![json!({
            "end": true,
            "err": { "message": other.to_string(), "status": error_status(other) }
        })],
    }
}

/// The client→server cancel notice for an in-flight streaming reply on a
/// broker transport, published to the transport's cancel channel. TCP and UDP
/// send `{"id": …, "cancel": true}` in-band instead.
pub fn frame_cancel(key: &str) -> serde_json::Value {
    json!({ "cancel": true, "key": key })
}

/// One reply frame as a streaming-aware client reads it.
#[derive(Debug)]
pub enum ReplyFrame {
    /// A single-reply envelope — `{"response":…}`, `{"err":…}`, or the
    /// raw-`Binary` fallback. A stream call answered this way is one item
    /// followed by an end.
    Single(Result<RpcData, RpcClientError>),
    /// One stream item.
    Item(RpcData),
    /// The clean end of a stream.
    End,
    /// The error end of a stream.
    EndErr {
        /// The error's display text.
        message: String,
        /// The wire status — `not_found` / `forbidden` / `error`.
        status: String,
    },
}

/// Parse one reply frame of a streaming-aware call. A frame outside the
/// stream grammar falls back to [`parse_response`]'s reading, so a
/// single-reply server still answers a stream call.
pub fn parse_reply_frame(bytes: &[u8]) -> ReplyFrame {
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return ReplyFrame::Single(Ok(RpcData::Binary(bytes.to_vec())));
    };
    if let Some(item) = v.get("stream") {
        return ReplyFrame::Item(RpcData::json(item.clone()));
    }
    if let Some(b64) = v.get("stream_b64").and_then(|s| s.as_str()) {
        return match BASE64.decode(b64) {
            Ok(b) => ReplyFrame::Item(RpcData::Binary(b)),
            Err(_) => ReplyFrame::EndErr {
                message: "invalid stream_b64 frame".to_string(),
                status: "error".to_string(),
            },
        };
    }
    if v.get("end").and_then(|e| e.as_bool()) == Some(true) {
        return match v.get("err") {
            Some(err) => ReplyFrame::EndErr {
                message: err
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("unknown error")
                    .to_string(),
                status: err
                    .get("status")
                    .and_then(|s| s.as_str())
                    .unwrap_or("error")
                    .to_string(),
            },
            None => ReplyFrame::End,
        };
    }
    ReplyFrame::Single(parse_response(bytes))
}

/// Drain a handler's reply stream through a transport's frame sender,
/// speaking the stream grammar: one item frame per element, the closing
/// frames from [`frame_stream_error`] on an `Err` item, the end marker on
/// exhaustion. An `Err` from `send` stops the drain and drops the stream
/// un-drained — the execution's cancellation token fires, so a producer
/// feeding an unreachable caller stops.
pub async fn drive_reply_stream<F, Fut, E>(
    mut stream: BoxStream<'static, Result<RpcData, RpcError>>,
    mut send: F,
) where
    F: FnMut(serde_json::Value) -> Fut,
    Fut: Future<Output = Result<(), E>>,
{
    while let Some(item) = stream.next().await {
        match item {
            Ok(data) => {
                if send(frame_stream_item(&data)).await.is_err() {
                    return;
                }
            }
            Err(e) => {
                for frame in frame_stream_error(&e) {
                    if send(frame).await.is_err() {
                        return;
                    }
                }
                return;
            }
        }
    }
    let _ = send(frame_stream_end()).await;
}

type CancelActions = Arc<Mutex<HashMap<String, Box<dyn FnOnce() + Send>>>>;

/// The streaming calls in flight on one connection or adapter, keyed by
/// correlation.
///
/// The registered action aborts the call's driving task — the adapter wraps
/// its own runtime's abort handle in the closure, which keeps the runtime out
/// of this crate. Aborting the task drops the handler future or the scoped
/// stream, either of which fires the execution's cancellation token.
#[derive(Clone, Default)]
pub struct Inflight {
    inner: CancelActions,
}

impl Inflight {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a call. Keep the returned guard alive inside the driving task
    /// for the call's lifetime: it removes the entry when the task finishes —
    /// or is aborted.
    pub fn register(
        &self,
        key: impl Into<String>,
        on_cancel: impl FnOnce() + Send + 'static,
    ) -> InflightGuard {
        let key = key.into();
        self.inner.lock().insert(key.clone(), Box::new(on_cancel));
        InflightGuard {
            inner: self.inner.clone(),
            key,
        }
    }

    /// Run one call's cancel action. `false` when nothing is registered under
    /// the key — the call already finished, or the notice was foreign.
    pub fn cancel(&self, key: &str) -> bool {
        let action = self.inner.lock().remove(key);
        match action {
            Some(action) => {
                action();
                true
            }
            None => false,
        }
    }

    /// Run every registered cancel action — the connection died or the
    /// adapter is shutting down.
    pub fn cancel_all(&self) {
        let actions: Vec<_> = {
            let mut map = self.inner.lock();
            map.drain().map(|(_, action)| action).collect()
        };
        for action in actions {
            action();
        }
    }
}

/// Removes its call's [`Inflight`] entry on drop.
pub struct InflightGuard {
    inner: CancelActions,
    key: String,
}

impl Drop for InflightGuard {
    fn drop(&mut self) {
        self.inner.lock().remove(&self.key);
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::sync::Arc;

    use super::*;
    use crate::{Error, ErrorKind};

    #[derive(Debug)]
    struct Teapot;

    impl std::fmt::Display for Teapot {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "teapot")
        }
    }

    impl std::error::Error for Teapot {}

    impl Error for Teapot {
        fn kind(&self) -> ErrorKind {
            ErrorKind::Conflict
        }

        fn message(&self) -> Cow<'_, str> {
            Cow::Borrowed("teapot")
        }
    }

    #[test]
    fn json_response_frames_into_the_response_envelope() {
        let bytes = frame_response(Ok(RpcHandlerOutput::Single(RpcData::json(
            serde_json::json!({"sum": 5}),
        ))))
        .into_bytes();
        assert_eq!(bytes, br#"{"response":{"sum":5}}"#);
    }

    #[test]
    fn text_response_frames_as_a_json_string() {
        let bytes = frame_response(Ok(RpcHandlerOutput::Single(RpcData::text("hi")))).into_bytes();
        assert_eq!(bytes, br#"{"response":"hi"}"#);
    }

    #[test]
    fn binary_response_passes_through_raw() {
        let bytes = frame_response(Ok(RpcHandlerOutput::Single(RpcData::binary(vec![1, 2, 3]))))
            .into_bytes();
        assert_eq!(bytes, vec![1, 2, 3]);
    }

    #[test]
    fn event_ack_is_a_null_response() {
        let bytes = frame_response(Ok(RpcHandlerOutput::Empty)).into_bytes();
        assert_eq!(bytes, br#"{"response":null}"#);
    }

    #[test]
    fn app_error_rides_the_response_lane() {
        let outcome = Err(RpcError::AppError(Arc::new(Teapot)));
        let v = frame_response(outcome).into_json_value();
        assert_eq!(v["response"]["status"], "error");
        assert_eq!(v["response"]["kind"], "Conflict");
        assert_eq!(v["response"]["message"], "teapot");
    }

    #[test]
    fn framework_error_frames_into_wire_err() {
        let v = frame_response(Err(RpcError::Forbidden("nope".into()))).into_json_value();
        assert_eq!(v["err"]["status"], "forbidden");
        assert_eq!(
            v["err"]["message"],
            RpcError::Forbidden("nope".into()).to_string()
        );
    }

    #[test]
    fn panic_frame_names_an_internal_error() {
        let bytes = frame_panic().into_bytes();
        assert_eq!(
            bytes,
            br#"{"err":{"message":"internal server error","status":"error"}}"#
        );
    }

    #[test]
    fn raw_frame_degrades_to_a_null_response_as_json() {
        let v = frame_response(Ok(RpcHandlerOutput::Single(RpcData::binary(vec![9]))))
            .into_json_value();
        assert_eq!(v, serde_json::json!({ "response": null }));
    }

    #[test]
    fn frame_then_parse_is_identity_for_json_response() {
        let bytes = frame_response(Ok(RpcHandlerOutput::Single(RpcData::json(
            serde_json::json!({"sum": 5}),
        ))))
        .into_bytes();
        let parsed = parse_response(&bytes).unwrap();
        assert_eq!(parsed.as_json(), Some(&serde_json::json!({"sum": 5})));
    }

    #[test]
    fn framework_error_parses_back_as_remote() {
        let bytes = frame_response(Err(RpcError::Forbidden("nope".into()))).into_bytes();
        match parse_response(&bytes) {
            Err(RpcClientError::Remote { status, .. }) => assert_eq!(status, "forbidden"),
            other => panic!("expected Remote error, got {other:?}"),
        }
    }

    #[test]
    fn unenveloped_json_parses_as_data() {
        let parsed = parse_response(br#"{"a":1}"#).unwrap();
        assert_eq!(parsed.as_json(), Some(&serde_json::json!({"a": 1})));
    }

    #[test]
    fn non_json_parses_as_binary() {
        match parse_response(&[0x01, 0x02]).unwrap() {
            RpcData::Binary(b) => assert_eq!(b, vec![0x01, 0x02]),
            other => panic!("expected Binary, got {other:?}"),
        }
    }

    #[test]
    fn stream_refusal_names_the_unsupported_transport() {
        let stream = futures::stream::empty().boxed();
        let v = frame_response(Ok(RpcHandlerOutput::Stream(stream))).into_json_value();
        assert_eq!(v["err"]["status"], "unsupported");
    }

    #[test]
    fn stream_item_frames_under_the_stream_key() {
        assert_eq!(
            frame_stream_item(&RpcData::json(serde_json::json!({"n": 1}))).to_string(),
            r#"{"stream":{"n":1}}"#
        );
        assert_eq!(
            frame_stream_item(&RpcData::text("hi")).to_string(),
            r#"{"stream":"hi"}"#
        );
    }

    #[test]
    fn binary_stream_item_round_trips_through_base64() {
        let frame = frame_stream_item(&RpcData::binary(vec![0, 159, 146, 150]));
        match parse_reply_frame(frame.to_string().as_bytes()) {
            ReplyFrame::Item(RpcData::Binary(b)) => assert_eq!(b, vec![0, 159, 146, 150]),
            other => panic!("expected Binary item, got {other:?}"),
        }
    }

    #[test]
    fn stream_end_is_the_end_marker() {
        assert_eq!(frame_stream_end().to_string(), r#"{"end":true}"#);
    }

    #[test]
    fn app_error_mid_stream_is_a_final_item_then_a_clean_end() {
        let frames = frame_stream_error(&RpcError::AppError(Arc::new(Teapot)));
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0]["stream"]["kind"], "Conflict");
        assert_eq!(frames[1], serde_json::json!({ "end": true }));
    }

    #[test]
    fn framework_error_mid_stream_is_an_error_end() {
        let frames = frame_stream_error(&RpcError::Internal("boom".into()));
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0]["end"], true);
        assert_eq!(frames[0]["err"]["status"], "error");
    }

    #[test]
    fn cancel_notice_carries_its_correlation() {
        assert_eq!(
            frame_cancel("inbox.7").to_string(),
            r#"{"cancel":true,"key":"inbox.7"}"#
        );
    }

    #[test]
    fn reply_frames_parse_by_kind() {
        assert!(matches!(
            parse_reply_frame(br#"{"stream":{"n":1}}"#),
            ReplyFrame::Item(_)
        ));
        assert!(matches!(
            parse_reply_frame(br#"{"end":true}"#),
            ReplyFrame::End
        ));
        match parse_reply_frame(br#"{"end":true,"err":{"message":"m","status":"forbidden"}}"#) {
            ReplyFrame::EndErr { status, .. } => assert_eq!(status, "forbidden"),
            other => panic!("expected EndErr, got {other:?}"),
        }
        assert!(matches!(
            parse_reply_frame(br#"{"response":{"n":1}}"#),
            ReplyFrame::Single(Ok(_))
        ));
        assert!(matches!(
            parse_reply_frame(&[0x01]),
            ReplyFrame::Single(Ok(RpcData::Binary(_)))
        ));
    }

    #[test]
    fn drive_sends_items_then_the_end_marker() {
        let stream = futures::stream::iter(vec![
            Ok(RpcData::json(serde_json::json!(1))),
            Ok(RpcData::json(serde_json::json!(2))),
        ])
        .boxed();
        let frames = std::sync::Arc::new(Mutex::new(Vec::new()));
        let sink = frames.clone();
        futures_executor::block_on(drive_reply_stream(stream, move |frame| {
            sink.lock().push(frame);
            async { Ok::<(), ()>(()) }
        }));
        let frames = frames.lock();
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[0], serde_json::json!({ "stream": 1 }));
        assert_eq!(frames[2], serde_json::json!({ "end": true }));
    }

    #[test]
    fn drive_stops_at_an_error_item() {
        let stream = futures::stream::iter(vec![
            Ok(RpcData::json(serde_json::json!(1))),
            Err(RpcError::Internal("boom".into())),
            Ok(RpcData::json(serde_json::json!(2))),
        ])
        .boxed();
        let frames = std::sync::Arc::new(Mutex::new(Vec::new()));
        let sink = frames.clone();
        futures_executor::block_on(drive_reply_stream(stream, move |frame| {
            sink.lock().push(frame);
            async { Ok::<(), ()>(()) }
        }));
        let frames = frames.lock();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[1]["end"], true);
        assert_eq!(frames[1]["err"]["status"], "error");
    }

    #[test]
    fn drive_stops_when_the_sender_fails() {
        let stream = futures::stream::iter(vec![
            Ok(RpcData::json(serde_json::json!(1))),
            Ok(RpcData::json(serde_json::json!(2))),
        ])
        .boxed();
        let sent = std::sync::Arc::new(Mutex::new(0u32));
        let counter = sent.clone();
        futures_executor::block_on(drive_reply_stream(stream, move |_| {
            *counter.lock() += 1;
            async { Err::<(), ()>(()) }
        }));
        assert_eq!(*sent.lock(), 1);
    }

    #[test]
    fn inflight_cancel_runs_the_action_once() {
        let inflight = Inflight::new();
        let fired = std::sync::Arc::new(Mutex::new(0u32));
        let counter = fired.clone();
        let guard = inflight.register("k", move || *counter.lock() += 1);
        assert!(inflight.cancel("k"));
        assert!(!inflight.cancel("k"));
        assert_eq!(*fired.lock(), 1);
        drop(guard);
    }

    #[test]
    fn inflight_guard_removes_a_finished_call() {
        let inflight = Inflight::new();
        let guard = inflight.register("k", || panic!("finished call must not cancel"));
        drop(guard);
        assert!(!inflight.cancel("k"));
    }
}
