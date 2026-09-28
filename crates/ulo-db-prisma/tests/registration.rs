//! How a Prisma client reaches an injectable, and what a second one of the
//! same type costs.
//!
//! No server is contacted. `for_root` takes a closure producing the generated
//! client, so any `Send + Sync + Clone` type stands in for one, and every
//! claim below is about registration rather than about Prisma.
//!
//! This is the whole of the crate's behaviour: it has no startup check and no
//! health indicator, which is why it shares neither suite with the other five
//! database integrations.

use std::sync::atomic::{AtomicUsize, Ordering};

use ulo::{UloFactory, injectable, module};
/// Stands in for the generated `db::PrismaClient`.
#[derive(Clone)]
struct FakeClient {
    url: &'static str,
}

static CONNECTS: AtomicUsize = AtomicUsize::new(0);

#[injectable]
struct ByType {
    #[inject]
    client: FakeClient,
}

#[module(imports: [PrismaModule::for_root(|| async {
    CONNECTS.fetch_add(1, Ordering::SeqCst);
    FakeClient { url: "primary" }
})], providers: [ByType], exports: [ByType])]
struct TypeModule {}

use ulo_db_prisma::PrismaModule;

#[tokio::test]
async fn a_client_injects_by_its_concrete_type() {
    CONNECTS.store(0, Ordering::SeqCst);

    let ctx = UloFactory::create_application_context(TypeModule)
        .await
        .expect("a module with one client starts");

    let svc = ctx.get::<ByType>().await.expect("the injectable resolves");
    assert_eq!(
        svc.client.url, "primary",
        "the client the closure produced must be the one injected"
    );
}

ulo::key!(Analytics: FakeClient);
ulo::key!(Secondary: FakeClient);

#[injectable]
struct ByName {
    #[inject(Analytics)]
    client: FakeClient,
}

#[module(imports: [PrismaModule::for_root_keyed::<Analytics, _>(|| async {
    FakeClient { url: "analytics" }
})], providers: [ByName], exports: [ByName])]
struct NamedModule {}

#[tokio::test]
async fn a_keyed_client_injects_by_its_marker() {
    let ctx = UloFactory::create_application_context(NamedModule)
        .await
        .expect("a module with one keyed client starts");

    let svc = ctx.get::<ByName>().await.expect("the injectable resolves");
    assert_eq!(
        svc.client.url, "analytics",
        "a keyed client is reached by its marker, not by its type"
    );
}

#[injectable]
struct BothClients {
    #[inject]
    default: FakeClient,
    #[inject(Secondary)]
    keyed: FakeClient,
}

#[module(imports: [
    PrismaModule::for_root(|| async { FakeClient { url: "primary" } }),
    PrismaModule::for_root_keyed::<Secondary, _>(|| async { FakeClient { url: "secondary" } }),
], providers: [BothClients], exports: [BothClients])]
struct TwoClientModule {}

/// The documented way to run two clients of one type: the second sits under a
/// marker, because the type alone no longer tells them apart.
#[tokio::test]
async fn a_second_client_of_one_type_is_reached_by_its_marker() {
    let ctx = UloFactory::create_application_context(TwoClientModule)
        .await
        .expect("a default client alongside a keyed one starts");

    let svc = ctx
        .get::<BothClients>()
        .await
        .expect("the injectable resolves");
    assert_eq!(svc.default.url, "primary");
    assert_eq!(svc.keyed.url, "secondary");
}

/// Two clients of one type without a marker are accepted, and the one written
/// last is the one injected.
///
/// `for_root_keyed`'s documentation says a marker is required to register more
/// than one client of the same type. Nothing enforces that: the two render one
/// module identity, having no `identity_hint` to tell them apart, the container
/// keeps the first module the scan reaches and drops the other as a repeat, and
/// startup reports nothing. The scan reaches the import written last first. The
/// other five database integrations refuse this at startup and name both
/// modules.
///
/// Pinned as what happens today rather than as what should; this test is what
/// will fail when the gap closes.
#[tokio::test]
async fn two_unnamed_clients_of_one_type_keep_the_last() {
    #[injectable]
    struct Solo {
        #[inject]
        client: FakeClient,
    }

    #[module(imports: [
        PrismaModule::for_root(|| async { FakeClient { url: "first" } }),
        PrismaModule::for_root(|| async { FakeClient { url: "second" } }),
    ], providers: [Solo], exports: [Solo])]
    struct TwoUnnamedModule {}

    let ctx = UloFactory::create_application_context(TwoUnnamedModule)
        .await
        .expect("two unnamed clients of one type are accepted today");

    let svc = ctx.get::<Solo>().await.expect("the injectable resolves");
    assert_eq!(
        svc.client.url, "second",
        "the later registration replaces the earlier one"
    );
}
