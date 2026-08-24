//! A connection that cannot be established fails startup with a returned error naming the module.
#![cfg(feature = "postgres")]

use toni::{StartupError, ToniFactory};
use toni_sqlx::SqlxModule;

#[tokio::test]
async fn a_connection_that_cannot_be_established_fails_startup() {
    let err = ToniFactory::create_application_context(SqlxModule::postgres(
        // A port outside the valid range fails while the URL is parsed, before any socket:
        // a refused connection would instead be retried until sqlx's 30-second acquire timeout.
        "postgres://someone:secret@127.0.0.1:99999/app",
    ))
    .await
    .err()
    .expect("an unusable connection string must fail startup");

    let StartupError::HookFailed { module, hook, .. } = &err else {
        panic!("expected HookFailed, got: {err}");
    };
    assert_eq!(*hook, "on_module_init");
    assert!(
        module.contains("SqlxModule"),
        "the failure should name the module, got: {module}"
    );

    let rendered = err.to_string();
    assert!(
        !rendered.contains("secret"),
        "the failure must not echo the connection string, got: {rendered}"
    );
}
