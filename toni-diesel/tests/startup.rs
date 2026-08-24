//! deadpool opens no connection when it builds a pool, so there is no reachable failure to report
//! here. What this pins is the other half: the startup hook returns `Ok` and does not turn a
//! healthy pool into a failed startup.
#![cfg(feature = "postgres")]

use toni::ToniFactory;
use toni_diesel::DieselModule;

#[tokio::test]
async fn a_pool_that_builds_starts_normally() {
    ToniFactory::create_application_context(DieselModule::postgres(
        "postgres://someone:secret@127.0.0.1:1/app",
    ))
    .await
    .map(|_| ())
    .expect("building a pool touches no server, so startup must succeed");
}
