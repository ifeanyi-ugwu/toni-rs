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

use serde_json::json;

use super::{RpcClientError, RpcData, RpcError};

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
pub fn frame_response(outcome: Result<Option<RpcData>, RpcError>) -> ResponseFrame {
    match outcome {
        Ok(Some(RpcData::Binary(b))) => ResponseFrame::Raw(b),
        Ok(Some(RpcData::Json(v))) => ResponseFrame::Json(json!({ "response": v })),
        Ok(Some(RpcData::Text(s))) => ResponseFrame::Json(json!({ "response": s })),
        // #[event_pattern] handler but the caller expects a reply — an ack
        // closes the pending request instead of timing it out.
        Ok(None) => ResponseFrame::Json(json!({ "response": null })),
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
        let bytes =
            frame_response(Ok(Some(RpcData::json(serde_json::json!({"sum": 5}))))).into_bytes();
        assert_eq!(bytes, br#"{"response":{"sum":5}}"#);
    }

    #[test]
    fn text_response_frames_as_a_json_string() {
        let bytes = frame_response(Ok(Some(RpcData::text("hi")))).into_bytes();
        assert_eq!(bytes, br#"{"response":"hi"}"#);
    }

    #[test]
    fn binary_response_passes_through_raw() {
        let bytes = frame_response(Ok(Some(RpcData::binary(vec![1, 2, 3])))).into_bytes();
        assert_eq!(bytes, vec![1, 2, 3]);
    }

    #[test]
    fn event_ack_is_a_null_response() {
        let bytes = frame_response(Ok(None)).into_bytes();
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
        let v = frame_response(Ok(Some(RpcData::binary(vec![9])))).into_json_value();
        assert_eq!(v, serde_json::json!({ "response": null }));
    }

    #[test]
    fn frame_then_parse_is_identity_for_json_response() {
        let bytes =
            frame_response(Ok(Some(RpcData::json(serde_json::json!({"sum": 5}))))).into_bytes();
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
}
