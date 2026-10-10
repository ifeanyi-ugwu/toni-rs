//! The runtime through the app: `AppBuilder::runtime` binds it as `Dep<dyn Runtime>` and as
//! `Dep<dyn Timer>`, one object, which `AppHandle` hands out too, and a task spawned through it
//! has the app's secrets redacted from its panic's message.

use std::future::pending;
use std::sync::Arc;

use futures::channel::oneshot;
use ulo::{App, Module, ModuleDef, ModuleIdentity, Runtime, Secret, Signal, Spawn, TaskEnd, Timer};

use super::{Recorder, within};

struct Empty;

impl Module for Empty {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, _m: &mut ModuleDef<'_>) {}
}

/// The secret [`Holding`] registers.
const SECRET: &str = "suite-registered-token-7f3a";

/// What a task spawned for [`redacted`] panics with: the registered secret, and a URL carrying a
/// password.
const SECRET_PANIC: &str =
    "ulo-runtime-conformance: a deliberate panic carrying suite-registered-token-7f3a and postgres://suite:hunter2@db.invalid/app";

/// What the state of a task spawned for [`redacted`] panics with while its aborted future is
/// dropped, beginning with [`DROP_MARKER`].
const DROP_SECRET_PANIC: &str = "ulo-runtime-conformance: a deliberate panic while an app task's aborted future drops, carrying suite-registered-token-7f3a";

/// The part of [`DROP_SECRET_PANIC`] the scenario finds the warning by.
const DROP_MARKER: &str = "a deliberate panic while an app task's aborted future drops";

/// Panics with [`DROP_SECRET_PANIC`] in its drop.
struct PanicsWithTheSecretWhenDropped;

impl Drop for PanicsWithTheSecretWhenDropped {
    fn drop(&mut self) {
        panic!("{DROP_SECRET_PANIC}");
    }
}

/// A module registering [`SECRET`] with the graph.
struct Holding(Secret<String>);

impl Module for Holding {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.secret(&self.0);
    }
}

/// Where `value` lives, whatever trait object points at it.
fn address<T: ?Sized>(value: &T) -> *const () {
    value as *const T as *const ()
}

pub async fn one_object<R: Runtime>(rt: R) {
    let app = App::builder(Empty)
        .runtime(rt)
        .wire()
        .expect("an empty app given a runtime did not wire")
        .connect()
        .await
        .expect("an empty app given a runtime did not connect");
    let runtime = app.get::<dyn Runtime>().await.expect("`Dep<dyn Runtime>` on an app given a runtime");
    let timer = app.get::<dyn Timer>().await.expect("`Dep<dyn Timer>` on an app given a runtime");
    let at = address(&*runtime);
    assert_eq!(address(&*timer), at, "`Dep<dyn Timer>` is another object than `Dep<dyn Runtime>`");
    let handle = app.handle();
    assert_eq!(handle.runtime().map(|runtime| address(&**runtime)), Some(at), "`AppHandle::runtime`");
    assert_eq!(handle.timer().map(|timer| address(&**timer)), Some(at), "`AppHandle::timer`");
    let task = runtime.spawn(Box::pin(async {}));
    let end = within(&*timer, task).await;
    assert!(end.as_ref().is_some_and(TaskEnd::is_finished), "a task spawned through `Dep<dyn Runtime>` ended {end:?}");
    app.close(Signal::new("conformance")).await.expect("the app did not close");
}

/// The text of the end of a task that panicked with [`SECRET_PANIC`], spawned on `spawner`.
async fn panic_text(spawner: &dyn Spawn, timer: &dyn Timer, through: &str) -> String {
    let task = spawner.spawn(Box::pin(async { panic!("{SECRET_PANIC}") }));
    match within(timer, task).await {
        Some(TaskEnd::Panicked(message)) => message.to_string(),
        other => panic!("a task spawned through {through} that panicked ended {other:?}"),
    }
}

/// A task spawned through the app's runtime, `Dep<dyn Runtime>` or `AppHandle::runtime()`, has
/// every secret the app registered replaced in its panic's message, and the userinfo stripped; so
/// does the `warn` logging a panic raised while its aborted future drops. A task spawned on the
/// runtime before any app holds it has only the userinfo stripped.
pub async fn redacted<R: Runtime>(rt: R) {
    let recorder = Recorder::installed();
    let bare = panic_text(&rt, &rt, "the bare runtime").await;
    assert!(
        bare.contains(SECRET) && bare.contains("postgres://[redacted]@db.invalid/app") && !bare.contains("hunter2"),
        "a task spawned on the bare runtime: the userinfo stripped and nothing else, got {bare:?}"
    );

    let app = App::builder(Holding(Secret::new(SECRET.to_owned())))
        .runtime(rt)
        .wire()
        .expect("an app registering a secret did not wire")
        .connect()
        .await
        .expect("an app registering a secret did not connect");
    let runtime = app.get::<dyn Runtime>().await.expect("`Dep<dyn Runtime>` on an app given a runtime");
    let handle = app.handle();
    let through_handle = Arc::clone(handle.runtime().expect("`AppHandle::runtime` on an app given a runtime"));
    for (through, spawner) in [("`Dep<dyn Runtime>`", &*runtime), ("`AppHandle::runtime()`", &*through_handle)] {
        let text = panic_text(spawner, spawner, through).await;
        assert!(
            !text.contains(SECRET) && text.contains("carrying [redacted] and postgres://[redacted]@db.invalid/app"),
            "a task spawned through {through}: the app's secret replaced and the userinfo stripped, got {text:?}"
        );
    }

    let (report, started) = oneshot::channel::<()>();
    let task = runtime.spawn(Box::pin(async move {
        let _guard = PanicsWithTheSecretWhenDropped;
        let _ = report.send(());
        pending::<()>().await;
    }));
    within(&*runtime, started)
        .await
        .expect("the task did not start")
        .expect("the task's future was dropped before it started");
    task.abort();
    let end = within(&*runtime, task).await.expect("an aborted task whose future panics while dropped did not end");
    assert!(end.is_aborted(), "a task whose future panicked while dropped after an abort ended {end:?}");
    let logged = recorder.warnings_containing(DROP_MARKER);
    assert!(
        logged.len() == 1 && !logged[0].contains(SECRET) && logged[0].contains("carrying [redacted]"),
        "the `warn` for a panic while an app task's aborted future dropped, the app's secret replaced: {logged:?}"
    );
    app.close(Signal::new("conformance")).await.expect("the app did not close");
}
