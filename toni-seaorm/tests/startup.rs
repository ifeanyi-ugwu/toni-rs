//! A connection that cannot be established fails startup with a returned error naming the module.
//!
//! Hermetic, and deliberately a malformed URL rather than a dead port: sqlx retries a refused
//! connection until its 30-second `acquire_timeout`, and the reporting path under test is the same
//! either way.

use std::time::Duration;

use toni::{StartupError, ToniFactory};
use toni_seaorm::SeaOrmModule;

#[tokio::test]
async fn a_connection_that_cannot_be_established_fails_startup() {
    let err = ToniFactory::create_application_context(SeaOrmModule::for_root(
        "postgres-not-a-scheme://someone:secret@nowhere.invalid/nothing",
    ))
    .await
    .err()
    .expect("an unusable connection string must fail startup");

    let StartupError::HookFailed { module, hook, .. } = &err else {
        panic!("expected HookFailed, got: {err}");
    };
    assert_eq!(*hook, "on_module_init");
    assert!(
        module.contains("SeaOrmModule"),
        "the failure should name the module, got: {module}"
    );

    // The connection string carries credentials, so it must not reach the message.
    let rendered = err.to_string();
    assert!(
        !rendered.contains("secret"),
        "the failure must not echo the connection string, got: {rendered}"
    );
}

/// The check contacts the server, so an unreachable one fails startup on the configured schedule
/// rather than on sqlx's 30-second acquire timeout.
#[tokio::test]
async fn an_unreachable_server_fails_the_check_on_its_own_schedule() {
    let started = std::time::Instant::now();

    let err = ToniFactory::create_application_context(
        SeaOrmModule::for_root("postgres://someone:secret@127.0.0.1:1/app").with_startup_check(
            toni::StartupCheck::default()
                .attempts(2)
                .delay(Duration::from_millis(50))
                .timeout(Duration::from_millis(400)),
        ),
    )
    .await
    .err()
    .expect("an unreachable server must fail startup");

    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the check must not wait for the driver's own timeout, took {:?}",
        started.elapsed()
    );
    assert!(
        matches!(&err, StartupError::HookFailed { hook, .. } if *hook == "on_module_init"),
        "expected HookFailed, got: {err}"
    );
    assert!(
        !err.to_string().contains("secret"),
        "the failure must not echo the connection string, got: {err}"
    );
}

/// Dropping the check starts the application without contacting the server.
#[tokio::test]
async fn dropping_the_check_starts_without_contacting_the_server() {
    ToniFactory::create_application_context(
        SeaOrmModule::for_root("postgres://someone:secret@127.0.0.1:1/app").without_startup_check(),
    )
    .await
    .map(|_| ())
    .expect("an unchecked module must start regardless of the server");
}
