//! Scenarios: payloads.

use std::time::Duration;

use bytes::Bytes;
use ulo_transport::ErrorKind;

use crate::Broker;
use crate::cases::app::{BYTES, ECHO, Fixture, Mounts, failed, refused_at_startup, streams};

/// The payload size tried on a link that declares no `max_frame`: past the default limit of
/// every broker that has one.
const LARGE: usize = 8 * 1024 * 1024;

/// A binary payload round trip where the link declares `binary`, the pre-I/O refusal with `binary_unsupported` where not.
pub async fn binary<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    let payload = Bytes::from_static(&[0x00, 0x01, 0xfe, 0xff]);
    let outcome = fixture.rpc().request::<_, Bytes>(BYTES, &payload).timeout(Duration::from_secs(5)).await;
    let capabilities = fixture.capabilities();
    if capabilities.binary {
        assert_eq!(outcome.expect("the binary call is answered"), payload);
    } else {
        let error = failed(outcome, ErrorKind::BadRequest);
        assert_eq!(error.reason(), Some("binary_unsupported"));
        refused_at_startup(&fixture.broker, Mounts { streaming: streams(&capabilities), binary: true }).await;
    }
    fixture.stop().await;
}

/// An oversized payload, as `bad_request` with `payload_too_large`.
pub async fn oversized<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    match fixture.capabilities().max_frame {
        Some(limit) => {
            let text = "x".repeat(usize::try_from(limit).unwrap_or(usize::MAX - 1) + 1);
            let outcome = fixture.rpc().request::<_, String>(ECHO, &text).timeout(Duration::from_secs(30)).await;
            let error = failed(outcome, ErrorKind::BadRequest);
            assert_eq!(error.reason(), Some("payload_too_large"));
        }
        // A limit the link learns from its broker on connecting is not declared beforehand: the
        // large payload either travels or is refused the declared way.
        None => {
            let text = "x".repeat(LARGE);
            match fixture.rpc().request::<_, String>(ECHO, &text).timeout(Duration::from_secs(30)).await {
                Ok(echoed) => assert_eq!(echoed.len(), LARGE),
                Err(error) => {
                    assert_eq!(error.kind(), ErrorKind::BadRequest, "an oversized payload fails as `bad_request`: {error:?}");
                    assert_eq!(error.reason(), Some("payload_too_large"));
                }
            }
        }
    }
    fixture.stop().await;
}
