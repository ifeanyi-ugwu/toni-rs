//! Which of `Dep<dyn Timer>` and `Dep<dyn Runtime>` an app binds for each way its builder sets a
//! clock, the refusal of a test's override of either, and `listen()`'s refusal of an app that
//! binds a transport with no runtime. The binding with `.runtime(..)` alone is the conformance
//! suite's `the_app_binds_the_runtime_as_one_object`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use ulo::app::Bound as Serving;
use ulo::testing::TestApp;
use ulo::{
    App, AppBuilder, BoxError, Connected, DrainToken, LookupError, Module, ModuleDef, ModuleIdentity, Mounted, Runtime,
    RuntimeMissing, Server, Signal, StartupError, Timer, Transport, WiringError,
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
    let app = connect(App::builder(Empty).runtime(Tokio::current()).timer(ulo_tokio::Timer)).await;
    assert!(app.get::<dyn Runtime>().await.is_err(), "`Dep<dyn Runtime>` after `.timer(..)` replaced the runtime");
    assert!(app.handle().runtime().is_none(), "`AppHandle::runtime` after `.timer(..)` replaced the runtime");
    app.close(Signal::new("test")).await.expect("the app did not close");
}

#[tokio::test]
async fn a_runtime_after_a_timer_replaces_it() {
    let app = connect(App::builder(Empty).timer(ulo_tokio::Timer).runtime(Tokio::current())).await;
    let runtime = app.get::<dyn Runtime>().await.expect("`Dep<dyn Runtime>` after `.runtime(..)`");
    let timer = app.get::<dyn Timer>().await.expect("`Dep<dyn Timer>` after `.runtime(..)`");
    assert_eq!(address(&*timer), address(&*runtime), "`Dep<dyn Timer>` is the timer `.runtime(..)` replaced");
    app.close(Signal::new("test")).await.expect("the app did not close");
}

#[tokio::test]
async fn overriding_the_runtime_is_refused() {
    let wired = TestApp::of(Empty).runtime(Tokio::current()).override_value::<dyn Runtime>(Arc::new(Tokio::current())).wire();
    let Err(StartupError::Wiring(errors)) = wired else {
        panic!("an override of `dyn Runtime` wired");
    };
    assert!(
        errors.iter().any(|error| matches!(error, WiringError::RuntimeOverride { .. })),
        "the errors: {errors}"
    );
}

/// A transport with no handlers, for a server that binds nothing.
struct Bare;

impl Transport for Bare {
    const KEY: &'static str = "bare";
    type Cx = ();
    type Reply = ();
}

/// What a [`BareServer`] saw: whether `prepare` ran, and the address of the runtime it was handed.
#[derive(Default)]
struct Prepared {
    ran: AtomicBool,
    runtime: Mutex<Option<usize>>,
}

/// A server that binds nothing and records its `prepare`.
struct BareServer(Arc<Prepared>);

impl Server for BareServer {
    type Transport = Bare;

    async fn prepare(&mut self, mounted: Mounted<'_, Bare>) -> Result<(), BoxError> {
        self.0.ran.store(true, Ordering::SeqCst);
        *self.0.runtime.lock().unwrap_or_else(PoisonError::into_inner) = Some(address(&**mounted.runtime()) as usize);
        Ok(())
    }

    async fn bind(&mut self, _mounted: Mounted<'_, Bare>) -> Result<(), BoxError> {
        Ok(())
    }

    async fn serve(&self) -> Result<(), BoxError> {
        Ok(())
    }

    async fn drain(&self, _token: DrainToken) {}

    async fn close(&self) -> Result<(), BoxError> {
        Ok(())
    }
}

/// `listen()`'s answer for `builder` binding a [`BareServer`], and whether its `prepare` ran.
async fn listen_bare(builder: AppBuilder) -> (Result<App<Serving>, StartupError>, Arc<Prepared>) {
    let prepared = Arc::new(Prepared::default());
    let listened = connect(builder).await.bind(BareServer(Arc::clone(&prepared))).listen().await;
    (listened, prepared)
}

fn assert_runtime_missing(listened: Result<App<Serving>, StartupError>, prepared: &Prepared, what: &str) {
    let Err(StartupError::Bind { transport, source }) = listened else {
        panic!("{what} binding a transport was not refused at `listen()`");
    };
    assert_eq!(transport.to_string(), "Bare", "the refusal names the transport bound");
    assert!(source.downcast_ref::<RuntimeMissing>().is_some(), "the refusal is not a `RuntimeMissing`: {source}");
    let text = source.to_string();
    assert!(text.contains("no `Runtime`") && text.contains("`.runtime(..)`"), "the refusal does not name `.runtime(..)`: {text}");
    assert!(!prepared.ran.load(Ordering::SeqCst), "a server on {what} was prepared before the refusal");
}

#[tokio::test]
async fn an_app_with_a_timer_alone_is_refused_when_it_binds_a_transport() {
    let (listened, prepared) = listen_bare(App::builder(Empty).timer(ulo_tokio::Timer)).await;
    assert_runtime_missing(listened, &prepared, "an app with a timer alone");
}

#[tokio::test]
async fn an_app_without_a_clock_is_refused_when_it_binds_a_transport() {
    let (listened, prepared) = listen_bare(App::builder(Empty)).await;
    assert_runtime_missing(listened, &prepared, "an app without a clock");
}

#[tokio::test]
async fn a_runtime_is_the_one_a_server_is_handed() {
    let (listened, prepared) = listen_bare(App::builder(Empty).runtime(Tokio::current())).await;
    let app = listened.unwrap_or_else(|error| panic!("an app with a runtime binding a transport was refused: {error}"));
    let handed = *prepared.runtime.lock().unwrap_or_else(PoisonError::into_inner);
    let runtime = app.handle().runtime().map(|runtime| address(&**runtime) as usize);
    assert!(handed.is_some(), "the server was not prepared");
    assert_eq!(handed, runtime, "`Mounted::runtime` is not the app's runtime");
    app.serve(async { Signal::new("test") }).await.expect("the app did not shut down");
}

#[tokio::test]
async fn a_timer_alone_is_enough_for_an_app_that_binds_nothing() {
    let app = connect(App::builder(Empty).timer(ulo_tokio::Timer)).await;
    let app = app.listen().await.unwrap_or_else(|error| panic!("an app binding no transport was refused: {error}"));
    app.serve(async { Signal::new("test") }).await.expect("the app did not shut down");
}
