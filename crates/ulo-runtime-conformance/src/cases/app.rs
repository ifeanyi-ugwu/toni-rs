//! The runtime through the app: `AppBuilder::runtime` binds it as `Dep<dyn Runtime>` and as
//! `Dep<dyn Timer>`, one object, which `AppHandle` hands out too.

use ulo::{App, Module, ModuleDef, ModuleIdentity, Runtime, Signal, TaskEnd, Timer};

use super::within;

struct Empty;

impl Module for Empty {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, _m: &mut ModuleDef<'_>) {}
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
    assert_eq!(within(&*timer, task).await, Some(TaskEnd::Finished), "a task spawned through `Dep<dyn Runtime>`");
    app.close(Signal::new("conformance")).await.expect("the app did not close");
}
