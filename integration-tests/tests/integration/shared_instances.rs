//! A singleton and an execution-scoped provider hand out one instance, which every holder shares,
//! and a transient hands out a fresh one to each. A lookup and a factory parameter reach the
//! instance a field holds, and a slot holding a trait object hands out the instance its hooks ran
//! on.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

use ulo::di::Execution;
use ulo::{UloFactory, injectable, key, module, on_module_init, provide};

#[injectable]
pub struct Store {
    #[default(OnceLock::new())]
    ready: OnceLock<()>,
}

impl Store {
    #[on_module_init]
    async fn init(&self) -> ulo::di::InitResult {
        let _ = self.ready.set(());
        Ok(())
    }
}

#[injectable(scope = "execution")]
pub struct Call {}

#[injectable(scope = "transient")]
pub struct Draft {}

#[injectable]
pub struct Left {
    #[inject]
    store: Arc<Store>,
    #[inject]
    draft: Arc<Draft>,
    #[inject]
    plain_draft: Draft,
}

#[injectable]
pub struct Right {
    #[inject]
    store: Arc<Store>,
    #[inject]
    draft: Arc<Draft>,
    #[inject]
    plain_draft: Draft,
}

#[injectable(scope = "execution")]
pub struct Near {
    #[inject]
    call: Arc<Call>,
}

#[injectable(scope = "execution")]
pub struct Far {
    #[inject]
    call: Arc<Call>,
}

pub struct Tally(Arc<Store>);

pub trait Named: Send + Sync {
    fn ready(&self) -> bool;
}

impl Named for Store {
    fn ready(&self) -> bool {
        self.ready.get().is_some()
    }
}

key!(pub Tick: u32);

#[module(providers: [
    Store,
    Call,
    Draft,
    Left,
    Right,
    Near,
    Far,
    provide!(async |store: Arc<Store>| Tally(store)),
    provide!(dyn Named => Store),
    provide!(Tick => async || 7u32).per_execution(),
])]
struct Scopes;

#[tokio::test]
async fn a_scope_decides_how_many_instances_its_holders_share() {
    let ctx = UloFactory::create_application_context(Scopes)
        .await
        .unwrap();

    let left = ctx.get::<Left>().await.unwrap();
    let right = ctx.get::<Right>().await.unwrap();
    assert!(
        Arc::ptr_eq(&left.store, &right.store),
        "two holders of a singleton hold one instance"
    );

    let execution = Execution::standalone();
    let near = ctx.resolve::<Near>(&execution).await.unwrap();
    let far = ctx.resolve::<Far>(&execution).await.unwrap();
    assert!(
        Arc::ptr_eq(&near.call, &far.call),
        "two holders in one execution hold one instance"
    );

    assert!(
        !Arc::ptr_eq(&left.draft, &right.draft),
        "two holders of a transient hold one each"
    );
    // A transient hands out its value, which a plain field holds.
    let _ = (&left.plain_draft, &right.plain_draft);
}

#[tokio::test]
async fn a_holder_sees_what_the_instance_did_after_it_was_built() {
    let ctx = UloFactory::create_application_context(Scopes)
        .await
        .unwrap();

    // `Store`'s hook runs after `Left` is built from it.
    let left = ctx.get::<Left>().await.unwrap();
    assert!(left.store.ready.get().is_some());
}

#[tokio::test]
async fn a_lookup_hands_out_the_instance_a_field_holds() {
    let ctx = UloFactory::create_application_context(Scopes)
        .await
        .unwrap();
    let left = ctx.get::<Left>().await.unwrap();

    let store = ctx.get::<Store>().await.unwrap();
    assert!(Arc::ptr_eq(&store, &left.store));

    let module = ctx.get_module::<Scopes>().await.unwrap();
    assert!(Arc::ptr_eq(
        &module.get::<Store>().await.unwrap(),
        &left.store
    ));

    let execution = Execution::standalone();
    let near = ctx.resolve::<Near>(&execution).await.unwrap();
    let call = ctx.resolve::<Call>(&execution).await.unwrap();
    assert!(Arc::ptr_eq(&call, &near.call));
}

#[tokio::test]
async fn a_factory_parameter_is_the_shared_instance() {
    let ctx = UloFactory::create_application_context(Scopes)
        .await
        .unwrap();

    let tally = ctx.get::<Tally>().await.unwrap();
    let store = ctx.get::<Store>().await.unwrap();
    assert!(Arc::ptr_eq(&tally.0, &store));
}

#[tokio::test]
async fn a_per_execution_factory_is_one_instance_in_one_execution() {
    let ctx = UloFactory::create_application_context(Scopes)
        .await
        .unwrap();

    let execution = Execution::standalone();
    let first = ctx.resolve_key::<Tick>(&execution).await.unwrap();
    let again = ctx.resolve_key::<Tick>(&execution).await.unwrap();
    assert!(Arc::ptr_eq(&first, &again));

    let other = ctx
        .resolve_key::<Tick>(&Execution::standalone())
        .await
        .unwrap();
    assert!(!Arc::ptr_eq(&first, &other));
}

#[injectable]
pub struct ReadsNamed {
    #[inject]
    named: Arc<dyn Named>,
}

#[module(providers: [provide!(dyn Named => Store), ReadsNamed])]
struct UnderATraitObject;

#[tokio::test]
async fn a_type_bound_under_a_trait_object_hands_out_the_instance_its_hook_ran_on() {
    let ctx = UloFactory::create_application_context(UnderATraitObject)
        .await
        .unwrap();

    let holder = ctx.get::<ReadsNamed>().await.unwrap();
    assert!(holder.named.ready());
}

/// Nothing clones an injectable, so one carrying its own `impl Clone` conflicts with no derive.
#[injectable]
pub struct OwnClone {
    #[default(3)]
    value: u32,
}

impl Clone for OwnClone {
    fn clone(&self) -> Self {
        Self { value: self.value }
    }
}

#[module(providers: [OwnClone])]
struct WithOwnClone;

#[tokio::test]
async fn an_injectable_may_implement_clone_itself() {
    let ctx = UloFactory::create_application_context(WithOwnClone)
        .await
        .unwrap();

    let own = ctx.get::<OwnClone>().await.unwrap();
    assert_eq!(OwnClone::clone(&own).value, 3);
}

/// Nothing clones an injectable, so its fields need not be `Clone`.
#[injectable]
pub struct Counter {
    #[default(AtomicUsize::new(0))]
    count: AtomicUsize,
}

#[module(providers: [Counter])]
struct WithCounter;

#[tokio::test]
async fn an_injectable_needs_no_clone_field() {
    let ctx = UloFactory::create_application_context(WithCounter)
        .await
        .unwrap();

    ctx.get::<Counter>()
        .await
        .unwrap()
        .count
        .fetch_add(1, Ordering::SeqCst);
    ctx.get::<Counter>()
        .await
        .unwrap()
        .count
        .fetch_add(1, Ordering::SeqCst);
    assert_eq!(
        ctx.get::<Counter>()
            .await
            .unwrap()
            .count
            .load(Ordering::SeqCst),
        2
    );
}
