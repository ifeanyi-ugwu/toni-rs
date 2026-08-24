//! An unreachable server fails startup with a returned error naming the module.
//!
//! deadpool opens no connection when it builds a pool, so the startup check is what contacts the
//! server at all — without it this module started against a database that was not there and failed
//! on the first query instead.
#![cfg(feature = "postgres")]

use std::time::Duration;

use toni::{StartupCheck, StartupError, ToniFactory};
use toni_diesel::DieselModule;

#[tokio::test]
async fn an_unreachable_server_fails_startup() {
    // Bounds derived from the schedule rather than written as constants: a loose constant is a
    // guard that cannot fail for the reason it exists.
    let check = StartupCheck::default()
        .attempts(2)
        .delay(Duration::from_millis(50))
        .timeout(Duration::from_millis(400));
    let started = std::time::Instant::now();

    let err = ToniFactory::create_application_context(
        DieselModule::postgres("postgres://someone:secret@127.0.0.1:1/app")
            .with_startup_check(check),
    )
    .await
    .err()
    .expect("an unreachable server must fail startup");
    let elapsed = started.elapsed();

    // Upper: the driver must not be waiting on its own timeout, which is thirty seconds for the
    // pooled drivers and unbounded for redis with its internal retry left on.
    assert!(
        elapsed < check.worst_case() * 3,
        "the check must give up on its own schedule (worst case {:?}), took {elapsed:?}",
        check.worst_case()
    );
    // Lower: without the retry this fails on the first refused connection, well under one gap.
    assert!(
        elapsed >= check.retry_delay(),
        "the check must retry rather than fail on the first refusal, took {elapsed:?}"
    );
    assert!(
        matches!(&err, StartupError::HookFailed { hook, .. } if *hook == "on_module_init"),
        "expected HookFailed, got: {err}"
    );
}

#[tokio::test]
async fn dropping_the_check_starts_without_contacting_the_server() {
    ToniFactory::create_application_context(
        DieselModule::postgres("postgres://someone:secret@127.0.0.1:1/app").without_startup_check(),
    )
    .await
    .map(|_| ())
    .expect("an unchecked module must start regardless of the server");
}
