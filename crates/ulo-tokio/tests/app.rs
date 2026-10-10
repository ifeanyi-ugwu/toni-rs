//! Which of `Dep<dyn Timer>` and `Dep<dyn Runtime>` an app binds for each way its builder sets a
//! clock, the refusal of a test's override of either, and `listen()`'s refusal of an app that
//! binds a transport with no runtime. The binding with `.runtime(..)` alone is the conformance
//! suite's `the_app_binds_the_runtime_as_one_object`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use ulo::app::Bound as Serving;
use ulo::testing::TestApp;
use ulo::{
    App, AppBuilder, BoxError, BoxFuture, ConnectError, Connected, DrainToken, FailureReason, LookupError, Module,
    ModuleDef, ModuleIdentity, Mounted, Runtime, RuntimeMissing, Server, Signal, StartupError, TaskEnd, Timer, Transport,
    WiringError,
};
use ulo_tokio::Tokio;

/// How long a step that should be immediate may take before the test fails.
const PATIENCE: Duration = Duration::from_secs(5);

/// A test's clock: the time stands at one instant a day ahead, and every sleep ends at once.
struct Frozen(Instant);

impl Frozen {
    fn new() -> Frozen {
        Frozen(Instant::now() + Duration::from_secs(86_400))
    }
}

impl Timer for Frozen {
    fn sleep(&self, _d: Duration) -> BoxFuture<'static, ()> {
        Box::pin(async {})
    }

    fn now(&self) -> Instant {
        self.0
    }
}

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

/// `.runtime(r).timer(t)`: the app spawns on `r` and has `t` as its one clock. `Dep<dyn Timer>`
/// and `AppHandle::timer` are `t`; `Dep<dyn Runtime>` and `AppHandle::runtime` are one object
/// whose clock reads and sleeps on `t` and whose tasks run on `r`.
async fn assert_spawns_on_the_runtime_and_times_with_the_timer(app: App<Connected>, at: Instant) {
    let runtime = app.get::<dyn Runtime>().await.expect("`Dep<dyn Runtime>` after `.timer(..)` replaced its clock");
    let timer = app.get::<dyn Timer>().await.expect("`Dep<dyn Timer>` after `.timer(..)`");
    let handle = app.handle();
    assert_eq!(timer.now(), at, "`Dep<dyn Timer>` is the timer set after the runtime");
    assert_eq!(runtime.now(), at, "`Dep<dyn Runtime>`'s clock is the timer set after the runtime");
    assert_eq!(handle.timer().map(|timer| timer.now()), Some(at), "`AppHandle::timer`");
    assert_eq!(handle.runtime().map(|runtime| runtime.now()), Some(at), "`AppHandle::runtime`'s clock");
    assert_eq!(handle.timer().map(|timer| address(&**timer)), Some(address(&*timer)), "`AppHandle::timer` is `Dep<dyn Timer>`");
    assert_eq!(
        handle.runtime().map(|runtime| address(&**runtime)),
        Some(address(&*runtime)),
        "`AppHandle::runtime` is `Dep<dyn Runtime>`"
    );
    let slept = tokio::time::timeout(PATIENCE, runtime.sleep(Duration::from_secs(3600))).await;
    assert!(slept.is_ok(), "`Dep<dyn Runtime>` slept an hour on the runtime's own clock, not the timer's");
    let task = tokio::time::timeout(PATIENCE, runtime.spawn(Box::pin(async {}))).await;
    assert!(matches!(task, Ok(TaskEnd::Finished)), "a task spawned through `Dep<dyn Runtime>` ended {task:?}");
    app.close(Signal::new("test")).await.expect("the app did not close");
}

/// `.timer(t).runtime(r)`: `r` for both, one object.
async fn assert_one_object_on_the_runtime(app: App<Connected>, at: Instant) {
    let runtime = app.get::<dyn Runtime>().await.expect("`Dep<dyn Runtime>` after `.runtime(..)`");
    let timer = app.get::<dyn Timer>().await.expect("`Dep<dyn Timer>` after `.runtime(..)`");
    assert_eq!(address(&*timer), address(&*runtime), "`Dep<dyn Timer>` is the runtime set after the timer");
    assert_ne!(timer.now(), at, "`Dep<dyn Timer>` still reads the timer `.runtime(..)` replaced");
    let handle = app.handle();
    assert_eq!(handle.timer().map(|timer| address(&**timer)), Some(address(&*runtime)), "`AppHandle::timer`");
    assert_eq!(handle.runtime().map(|runtime| address(&**runtime)), Some(address(&*runtime)), "`AppHandle::runtime`");
    app.close(Signal::new("test")).await.expect("the app did not close");
}

#[tokio::test]
async fn a_timer_after_a_runtime_replaces_only_its_clock() {
    let frozen = Frozen::new();
    let at = frozen.0;
    let app = connect(App::builder(Empty).runtime(Tokio::current()).timer(frozen)).await;
    assert_spawns_on_the_runtime_and_times_with_the_timer(app, at).await;
}

#[tokio::test]
async fn a_runtime_after_a_timer_replaces_both() {
    let frozen = Frozen::new();
    let at = frozen.0;
    let app = connect(App::builder(Empty).timer(frozen).runtime(Tokio::current())).await;
    assert_one_object_on_the_runtime(app, at).await;
}

#[tokio::test]
async fn a_test_s_timer_after_its_runtime_replaces_only_its_clock() {
    let frozen = Frozen::new();
    let at = frozen.0;
    let app = TestApp::of(Empty).runtime(Tokio::current()).timer(frozen).connect().await.expect("the test app connects");
    assert_spawns_on_the_runtime_and_times_with_the_timer(app, at).await;
}

#[tokio::test]
async fn a_test_s_runtime_after_its_timer_replaces_both() {
    let frozen = Frozen::new();
    let at = frozen.0;
    let app = TestApp::of(Empty).timer(frozen).runtime(Tokio::current()).connect().await.expect("the test app connects");
    assert_one_object_on_the_runtime(app, at).await;
}

/// A module whose init hook never returns, under `hook_timeout`.
struct Hangs;

impl Module for Hangs {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.on_init(std::future::pending::<Result<(), BoxError>>);
    }
}

/// The app's own waits read the timer set after the runtime: an hour's `hook_timeout` expires at
/// once on a clock whose sleeps end at once, where the runtime's own clock would wait the hour.
#[tokio::test]
async fn the_app_s_waits_read_the_timer_set_after_the_runtime() {
    let builder = App::builder(Hangs).runtime(Tokio::current()).timer(Frozen::new()).hook_timeout(Duration::from_secs(3600));
    let wired = builder.wire().expect("the app wires");
    let connected = tokio::time::timeout(PATIENCE, wired.connect())
        .await
        .expect("the init hook ran on the runtime's own clock, not the timer's");
    let Err(StartupError::Connect(ConnectError::Hook { reason, .. })) = connected else {
        panic!("a hook that never returns did not fail the connect phase");
    };
    assert!(matches!(reason, FailureReason::TimedOut { .. }), "the hook failed otherwise: {reason}");
}

#[tokio::test]
async fn overriding_the_runtime_is_refused_naming_the_test_builder() {
    let wired = TestApp::of(Empty).runtime(Tokio::current()).override_value::<dyn Runtime>(Arc::new(Tokio::current())).wire();
    let Err(StartupError::Wiring(errors)) = wired else {
        panic!("an override of `dyn Runtime` wired");
    };
    assert!(
        errors.iter().any(|error| matches!(error, WiringError::RuntimeOverride { .. })),
        "the errors: {errors}"
    );
    let text = errors.to_string();
    assert!(text.contains("help: set it with `TestApp::runtime(..)`"), "the refusal's hint does not name the test builder: {text}");
}

#[tokio::test]
async fn overriding_the_timer_is_refused_naming_the_test_builder() {
    let wired = TestApp::of(Empty).timer(ulo_tokio::Timer).override_value::<dyn Timer>(Arc::new(ulo_tokio::Timer)).wire();
    let Err(StartupError::Wiring(errors)) = wired else {
        panic!("an override of `dyn Timer` wired");
    };
    assert!(errors.iter().any(|error| matches!(error, WiringError::TimerOverride { .. })), "the errors: {errors}");
    let text = errors.to_string();
    assert!(text.contains("help: set it with `TestApp::timer(..)`"), "the refusal's hint does not name the test builder: {text}");
}

/// A transport with no handlers, for a server that binds nothing.
struct Bare;

impl Transport for Bare {
    const KEY: &'static str = "bare";
    type Cx = ();
    type Reply = ();
}

/// What a [`BareServer`] saw: whether `prepare` ran, the address of the runtime it was handed, and
/// the time its timer and its runtime read.
#[derive(Default)]
struct Prepared {
    ran: AtomicBool,
    runtime: Mutex<Option<usize>>,
    clocks: Mutex<Option<(Instant, Instant)>>,
}

/// A server that binds nothing and records its `prepare`.
struct BareServer(Arc<Prepared>);

impl Server for BareServer {
    type Transport = Bare;

    async fn prepare(&mut self, mounted: Mounted<'_, Bare>) -> Result<(), BoxError> {
        self.0.ran.store(true, Ordering::SeqCst);
        *self.0.runtime.lock().unwrap_or_else(PoisonError::into_inner) = Some(address(&**mounted.runtime()) as usize);
        *self.0.clocks.lock().unwrap_or_else(PoisonError::into_inner) = Some((mounted.timer().now(), mounted.runtime().now()));
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

#[tokio::test]
async fn a_server_times_with_the_timer_set_after_the_runtime() {
    let frozen = Frozen::new();
    let at = frozen.0;
    let (listened, prepared) = listen_bare(App::builder(Empty).runtime(Tokio::current()).timer(frozen)).await;
    let app = listened.unwrap_or_else(|error| panic!("an app with a runtime binding a transport was refused: {error}"));
    let clocks = *prepared.clocks.lock().unwrap_or_else(PoisonError::into_inner);
    assert_eq!(clocks, Some((at, at)), "`Mounted::timer` and `Mounted::runtime` read the timer set after the runtime");
    app.serve(async { Signal::new("test") }).await.expect("the app did not shut down");
}
