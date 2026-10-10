//! The runtime suite's `warn` scenario in a test binary that installed its own global `tracing`
//! subscriber first: the suite's recorder cannot be installed, and the scenario fails saying so
//! rather than passing without having read the warning.

use std::panic::{AssertUnwindSafe, catch_unwind};

use ulo_runtime_conformance::cases::SUBSCRIBER_TAKEN;
use ulo_runtime_conformance::cases::task::panic_while_aborted;
use ulo_tokio::Tokio;

#[test]
fn the_warn_scenario_fails_where_the_binary_installed_its_own_subscriber() {
    tracing::subscriber::set_global_default(tracing::subscriber::NoSubscriber::default())
        .expect("the test binary's own subscriber is the first");
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("a tokio runtime for the scenario");
    let ran = catch_unwind(AssertUnwindSafe(|| runtime.block_on(async { panic_while_aborted(Tokio::current()).await })));
    let Err(payload) = ran else {
        panic!("the `warn` scenario passed in a binary whose own subscriber received the warning");
    };
    let message = payload.downcast_ref::<String>().map(String::as_str).or_else(|| payload.downcast_ref::<&str>().copied());
    assert_eq!(message, Some(SUBSCRIBER_TAKEN), "the scenario's failure");
}
