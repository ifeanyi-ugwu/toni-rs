//! A connection that cannot be established fails startup with a returned error naming the module.

use std::time::Duration;

use toni::{StartupCheck, StartupError, ToniFactory};
use toni_redis::RedisModule;

#[tokio::test]
async fn a_connection_that_cannot_be_established_fails_startup() {
    let err = ToniFactory::create_application_context(RedisModule::for_root(
        "not-a-scheme://someone:secret@127.0.0.1:1",
    ))
    .await
    .err()
    .expect("an unusable connection string must fail startup");

    let StartupError::HookFailed { module, hook, .. } = &err else {
        panic!("expected HookFailed, got: {err}");
    };
    assert_eq!(*hook, "on_module_init");
    assert!(
        module.contains("RedisModule"),
        "the failure should name the module, got: {module}"
    );

    let rendered = err.to_string();
    assert!(
        !rendered.contains("secret"),
        "the failure must not echo the connection string, got: {rendered}"
    );
}

/// The check contacts the server, so an unreachable one fails startup on the configured schedule
/// rather than on whatever the driver does by itself.
#[tokio::test]
async fn an_unreachable_server_fails_startup() {
    let started = std::time::Instant::now();

    let err = ToniFactory::create_application_context(
        RedisModule::for_root("redis://127.0.0.1:1").with_startup_check(
            StartupCheck::default()
                .attempts(2)
                .delay(Duration::from_millis(50))
                .timeout(Duration::from_millis(400)),
        ),
    )
    .await
    .err()
    .expect("an unreachable server must fail startup");

    assert!(
        started.elapsed() < Duration::from_secs(30),
        "the check must not wait for the driver's own timeout, took {:?}",
        started.elapsed()
    );
    // A lower bound as well as an upper one: without the retry this fails on the first refused
    // connection, and elapsed would be under the one 50ms gap the schedule asks for.
    assert!(
        started.elapsed() >= Duration::from_millis(50),
        "the check must retry rather than fail on the first refusal, took {:?}",
        started.elapsed()
    );
    assert!(
        matches!(&err, StartupError::HookFailed { hook, .. } if *hook == "on_module_init"),
        "expected HookFailed, got: {err}"
    );
}

/// Dropping the check starts the application without contacting the server.
#[tokio::test]
async fn dropping_the_check_starts_without_contacting_the_server() {
    ToniFactory::create_application_context(
        RedisModule::for_root("redis://127.0.0.1:1").without_startup_check(),
    )
    .await
    .map(|_| ())
    .expect("an unchecked module must start regardless of the server");
}
