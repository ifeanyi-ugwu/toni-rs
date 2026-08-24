//! An unreachable server fails startup with a returned error naming the module.
//!
//! deadpool opens no connection when it builds a pool, so the startup check is what contacts the
//! server at all — without it this module started against a database that was not there and failed
//! on the first query instead.
#![cfg(feature = "postgres")]

use std::time::Duration;

use toni::{StartupCheck, StartupError, ToniFactory};
use toni_diesel::DieselModule;

fn brisk() -> StartupCheck {
    StartupCheck::default()
        .attempts(2)
        .delay(Duration::from_millis(50))
        .timeout(Duration::from_millis(400))
}

#[tokio::test]
async fn an_unreachable_server_fails_startup() {
    let err = ToniFactory::create_application_context(
        DieselModule::postgres("postgres://someone:secret@127.0.0.1:1/app")
            .with_startup_check(brisk()),
    )
    .await
    .err()
    .expect("an unreachable server must fail startup");

    let StartupError::HookFailed { module, hook, .. } = &err else {
        panic!("expected HookFailed, got: {err}");
    };
    assert_eq!(*hook, "on_module_init");
    assert!(
        module.contains("DieselModule"),
        "the failure should name the module, got: {module}"
    );
    assert!(
        !err.to_string().contains("secret"),
        "the failure must not echo the connection string, got: {err}"
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
