//! A connection that cannot be established fails startup with a returned error naming the module.

use toni::{StartupError, ToniFactory};
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
