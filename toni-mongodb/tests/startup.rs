//! A client that cannot be constructed fails startup with a returned error naming the module.
//!
//! The driver connects lazily, so this covers what construction can check: the URI.

use toni::{StartupError, ToniFactory};
use toni_mongodb::MongoModule;

#[tokio::test]
async fn a_uri_that_cannot_be_parsed_fails_startup() {
    let err = ToniFactory::create_application_context(MongoModule::for_root(
        "not-a-scheme://someone:secret@127.0.0.1:1",
        "app",
    ))
    .await
    .err()
    .expect("an unusable URI must fail startup");

    let StartupError::HookFailed { module, hook, .. } = &err else {
        panic!("expected HookFailed, got: {err}");
    };
    assert_eq!(*hook, "on_module_init");
    assert!(
        module.contains("MongoModule"),
        "the failure should name the module, got: {module}"
    );

    let rendered = err.to_string();
    assert!(
        !rendered.contains("secret"),
        "the failure must not echo the URI, got: {rendered}"
    );
}
