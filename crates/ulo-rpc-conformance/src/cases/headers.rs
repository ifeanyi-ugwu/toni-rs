//! Scenarios: headers.

use std::time::Duration;

use crate::Broker;
use crate::cases::app::{Fixture, HEADER, HEADERS};

/// Headers the client sets reach `CallHeaders`.
pub async fn reach_call_headers<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    let seen = fixture
        .rpc()
        .request::<_, Option<String>>(HEADERS, &())
        .header(HEADER, "conformance-value")
        .timeout(Duration::from_secs(5))
        .await
        .expect("the headers call is answered");
    assert_eq!(seen.as_deref(), Some("conformance-value"));
    fixture.stop().await;
}
