//! A dependency that is not built yet is waited for; one that does not exist is refused.
//!
//! Providers are instantiated module by module, and a global provider becomes available only once
//! the module contributing it has been built. A module ordered before that one asks for something
//! that exists and is not ready, which is a wait rather than a failure: the loader sets it aside
//! and comes back on a later pass. Nothing about the wait is visible from outside — a retry that
//! stopped happening would surface as an application that refuses to start, naming a provider the
//! reader can see is declared.

use ulo::UloFactory;
use ulo::{injectable, module};
#[injectable]
pub struct SharedClock {
    #[default("tick".to_string())]
    pub label: String,
}

/// Contributes `SharedClock` to every module without being imported by any.
#[module(providers: [SharedClock], exports: [SharedClock], global: true)]
impl ClockModule {}

/// Depends on the global, and is imported ahead of the module that provides it.
#[injectable]
pub struct Scheduler {
    #[inject]
    clock: SharedClock,
}

impl Scheduler {
    pub fn describe(&self) -> String {
        format!("scheduled on {}", self.clock.label)
    }
}

#[module(providers: [Scheduler], exports: [Scheduler])]
impl SchedulerModule {}

/// `SchedulerModule` is first, so its provider is asked for before `ClockModule` has
/// contributed the global it needs.
#[module(imports: [SchedulerModule, ClockModule])]
impl ConsumerFirstModule {}

/// The application starts, which it cannot do unless the deferred module was tried again.
#[tokio::test]
async fn a_module_waiting_on_a_global_is_retried() {
    let app = UloFactory::create(ConsumerFirstModule)
        .await
        .expect("the module ordered before its global provider is retried, not refused");

    let scheduler = app
        .get_from::<Scheduler>(&ulo::di::token_of::<SchedulerModule>())
        .await
        .expect("the deferred module's provider is built by the time create returns");
    assert_eq!(scheduler.describe(), "scheduled on tick");
}

/// Importing the provider's module first does not change the outcome: a global is contributed
/// after its own module is built, so the consumer waits either way.
#[module(imports: [ClockModule, SchedulerModule])]
impl ProviderFirstModule {}

#[tokio::test]
async fn import_order_does_not_decide_whether_the_wait_happens() {
    let app = UloFactory::create(ProviderFirstModule).await.unwrap();

    let scheduler = app
        .get_from::<Scheduler>(&ulo::di::token_of::<SchedulerModule>())
        .await
        .unwrap();
    assert_eq!(scheduler.describe(), "scheduled on tick");
}

#[injectable]
pub struct Orphan {
    #[inject]
    missing: NeverProvided,
}

#[injectable]
pub struct NeverProvided {}

/// `NeverProvided` is declared nowhere, so no pass can build `Orphan`.
#[module(providers: [Orphan])]
impl OrphanModule {}

/// A dependency nothing declares is refused on the first pass, with no wait.
///
/// This is the other arm, and pairing it with the retry above is the point: a provider that is not
/// built yet and a provider that does not exist are told apart by which arm the loader takes. They
/// reach the reader as a started application and a named refusal, and confusing them means either
/// a startup that refuses work it could do, or one that retries until the pass budget is spent.
#[tokio::test]
async fn an_undeclared_dependency_is_refused_rather_than_waited_on() {
    let rendered = match UloFactory::create(OrphanModule).await {
        Err(err) => err.to_string(),
        Ok(_) => panic!("nothing provides the dependency"),
    };
    assert!(
        rendered.contains("NeverProvided"),
        "the refusal names what could not be resolved, got: {rendered}"
    );
}
