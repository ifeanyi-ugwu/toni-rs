//! Scenarios: host values.

use crate::wire::{Exchange, is_reference, start};
use crate::{HOST_VALUE_HEADER, Host, Mode};

/// `Host<T>` present, and absent as a 500, where the host declares `host_extensions`: the host's own
/// middleware writes the value into its request store, and the app reads it there. The reference
/// has no host to write it, so only the absent half runs against it.
pub async fn present_and_absent<H: Host>(mode: Mode) {
    if !H::limits().host_extensions {
        return;
    }
    let host = start::<H>(mode).await;
    if !is_reference::<H>() {
        let present = host.send(Exchange::get("/host-value").header(HOST_VALUE_HEADER, "frank")).await;
        assert_eq!(present.status, 200);
        assert_eq!(present.text(), "frank");
    }
    let absent = host.send(Exchange::get("/host-value")).await;
    assert_eq!(absent.status, 500, "a missing host value is a deployment fault");
    assert!(!absent.text().contains("HostValue"), "the 500 names the missing type in its body");
    host.stop().await;
}

/// An `Embedded::forward` copy reaching `Host<T>` on a host declaring `host_extensions: false`.
pub async fn forward_copy<H: Host>(mode: Mode) {
    if H::limits().host_extensions {
        return;
    }
    let host = start::<H>(mode).await;
    let present = host.send(Exchange::get("/host-value").header(HOST_VALUE_HEADER, "frank")).await;
    assert_eq!(present.status, 200);
    assert_eq!(present.text(), "frank");
    let absent = host.send(Exchange::get("/host-value")).await;
    assert_eq!(absent.status, 500, "a copy finding nothing inserts nothing");
    host.stop().await;
}
