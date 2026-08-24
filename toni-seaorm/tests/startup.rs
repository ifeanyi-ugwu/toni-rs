//! A connection that cannot be established fails startup with a returned error naming the module.
//!
//! Hermetic, and deliberately a malformed URL rather than a dead port: sqlx retries a refused
//! connection until its 30-second `acquire_timeout`, and the reporting path under test is the same
//! either way.

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
