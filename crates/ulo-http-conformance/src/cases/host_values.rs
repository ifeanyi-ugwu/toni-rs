//! Scenarios: host values.

use crate::wire::{Exchange, is_reference, start};
use crate::{HOST_VALUE_HEADER, Host, Mode};

/// `Host<T>` present, and absent as a 500. On a host declaring `host_extensions`, the host's
/// middleware writes the value into its own request store, which reaches the app; on one
/// declaring `false`, the host's middleware writes it into the store the host keeps, and an
/// `Embedded::forward` copy carries it across. The reference has no host to write it, so only the
/// absent half runs against it.
pub async fn present_and_absent<H: Host>(mode: Mode) {
    let host = start::<H>(mode).await;
    let through = if H::limits().host_extensions { "the host's request extensions" } else { "an `Embedded::forward` copy" };
    if !is_reference::<H>() {
        let present = host.send(Exchange::get("/host-value").header(HOST_VALUE_HEADER, "frank")).await;
        assert_eq!(present.status, 200, "a host value through {through}");
        assert_eq!(present.text(), "frank", "a host value through {through}");
    }
    let absent = host.send(Exchange::get("/host-value")).await;
    assert_eq!(absent.status, 500, "a missing host value, through {through}, is a deployment fault");
    assert!(!absent.text().contains("HostValue"), "the 500 names the missing type in its body");
    host.stop().await;
}
