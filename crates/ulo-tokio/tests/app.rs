//! Which of `Dep<dyn Timer>` and `Dep<dyn Runtime>` an app binds for each way its builder sets a
//! clock, and the refusal of a test's override of either. The binding with `.runtime(..)` alone is
//! the conformance suite's `the_app_binds_the_runtime_as_one_object`.

use std::sync::Arc;

use ulo::testing::TestApp;
use ulo::{
    App, AppBuilder, Connected, LookupError, Module, ModuleDef, ModuleIdentity, Runtime, Signal, StartupError, Timer,
    WiringError,
};
use ulo_tokio::Tokio;

struct Empty;

impl Module for Empty {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, _m: &mut ModuleDef<'_>) {}
}

async fn connect(builder: AppBuilder) -> App<Connected> {
    builder.wire().expect("an empty app did not wire").connect().await.expect("an empty app did not connect")
}

fn address<T: ?Sized>(value: &T) -> *const () {
    value as *const T as *const ()
}

#[tokio::test]
async fn a_timer_alone_binds_no_runtime() {
    let app = connect(App::builder(Empty).timer(ulo_tokio::Timer)).await;
    assert!(app.get::<dyn Timer>().await.is_ok(), "`Dep<dyn Timer>` on an app given a timer");
    let runtime = app.get::<dyn Runtime>().await.map(|_| ());
    assert!(
        matches!(runtime, Err(LookupError::NotFound { .. })),
        "`Dep<dyn Runtime>` on an app given a timer alone: {runtime:?}"
    );
    let handle = app.handle();
    assert!(handle.timer().is_some(), "`AppHandle::timer` on an app given a timer");
    assert!(handle.runtime().is_none(), "`AppHandle::runtime` on an app given a timer alone");
    app.close(Signal::new("test")).await.expect("the app did not close");
}

#[tokio::test]
async fn an_app_without_a_clock_hands_out_neither() {
    let app = connect(App::builder(Empty)).await;
    let handle = app.handle();
    assert!(handle.timer().is_none(), "`AppHandle::timer` on an app given no clock");
    assert!(handle.runtime().is_none(), "`AppHandle::runtime` on an app given no clock");
    app.close(Signal::new("test")).await.expect("the app did not close");
}

#[tokio::test]
async fn a_timer_after_a_runtime_replaces_it() {
    let app = connect(App::builder(Empty).runtime(Tokio).timer(ulo_tokio::Timer)).await;
    assert!(app.get::<dyn Runtime>().await.is_err(), "`Dep<dyn Runtime>` after `.timer(..)` replaced the runtime");
    assert!(app.handle().runtime().is_none(), "`AppHandle::runtime` after `.timer(..)` replaced the runtime");
    app.close(Signal::new("test")).await.expect("the app did not close");
}

#[tokio::test]
async fn a_runtime_after_a_timer_replaces_it() {
    let app = connect(App::builder(Empty).timer(ulo_tokio::Timer).runtime(Tokio)).await;
    let runtime = app.get::<dyn Runtime>().await.expect("`Dep<dyn Runtime>` after `.runtime(..)`");
    let timer = app.get::<dyn Timer>().await.expect("`Dep<dyn Timer>` after `.runtime(..)`");
    assert_eq!(address(&*timer), address(&*runtime), "`Dep<dyn Timer>` is the timer `.runtime(..)` replaced");
    app.close(Signal::new("test")).await.expect("the app did not close");
}

#[test]
fn overriding_the_runtime_is_refused() {
    let wired = TestApp::of(Empty).runtime(Tokio).override_value::<dyn Runtime>(Arc::new(Tokio)).wire();
    let Err(StartupError::Wiring(errors)) = wired else {
        panic!("an override of `dyn Runtime` wired");
    };
    assert!(
        errors.iter().any(|error| matches!(error, WiringError::RuntimeOverride { .. })),
        "the errors: {errors}"
    );
}
