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

/// Two clients of one type without a marker fail startup, naming both modules.
///
/// Neither closure can be compared with the other, so each `for_root` call is a
/// registration of its own: two modules exporting one client type globally,
/// which the container refuses as ADR-0029's global-export clash.
#[tokio::test]
async fn two_unnamed_clients_of_one_type_are_refused() {
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

    let err = match UloFactory::create_application_context(TwoUnnamedModule).await {
        Ok(_) => panic!("two unnamed clients of one type must be refused"),
        Err(err) => err.to_string(),
    };
    assert!(
        err.contains("exported globally by two modules"),
        "the refusal names the clash: {err}"
    );
    assert_eq!(
        err.matches("PrismaModule#").count(),
        2,
        "the refusal names both modules: {err}"
    );
}

/// Two clients under one marker fail startup the same way.
#[tokio::test]
async fn two_clients_under_one_marker_are_refused() {
    #[module(imports: [
        PrismaModule::for_root_keyed::<Analytics, _>(|| async { FakeClient { url: "first" } }),
        PrismaModule::for_root_keyed::<Analytics, _>(|| async { FakeClient { url: "second" } }),
    ])]
    struct TwoUnderOneMarker {}

    let err = match UloFactory::create_application_context(TwoUnderOneMarker).await {
        Ok(_) => panic!("two clients under one marker must be refused"),
        Err(err) => err.to_string(),
    };
    assert!(err.contains("exported globally by two modules"), "{err}");
}

/// Stands in for a second generated client type.
#[derive(Clone)]
struct OtherClient {
    url: &'static str,
}

/// Two client types register side by side, each module having an identity of
/// its own.
#[tokio::test]
async fn two_client_types_register_side_by_side() {
    #[injectable]
    struct BothTypes {
        #[inject]
        first: FakeClient,
        #[inject]
        second: OtherClient,
    }

    #[module(imports: [
        PrismaModule::for_root(|| async { FakeClient { url: "fake" } }),
        PrismaModule::for_root(|| async { OtherClient { url: "other" } }),
    ], providers: [BothTypes], exports: [BothTypes])]
    struct TwoTypesModule {}

    let ctx = UloFactory::create_application_context(TwoTypesModule)
        .await
        .expect("two client types start");
    let svc = ctx
        .get::<BothTypes>()
        .await
        .expect("the injectable resolves");
    assert_eq!(svc.first.url, "fake");
    assert_eq!(svc.second.url, "other");
}

static DIAMOND_CONNECTS: AtomicUsize = AtomicUsize::new(0);

#[module(imports: [PrismaModule::for_root(|| async {
    DIAMOND_CONNECTS.fetch_add(1, Ordering::SeqCst);
    FakeClient { url: "shared" }
})])]
struct DbModule {}

#[module(imports: [DbModule])]
struct UsersModule {}

#[module(imports: [DbModule, UsersModule], providers: [ByType], exports: [ByType])]
struct DiamondModule {}

/// A module importing Prisma, reached through two import paths, registers one
/// client: its imports are built once, not once per path.
#[tokio::test]
async fn a_module_reached_through_two_paths_registers_one_client() {
    DIAMOND_CONNECTS.store(0, Ordering::SeqCst);

    let ctx = UloFactory::create_application_context(DiamondModule)
        .await
        .expect("one client reached through two paths starts");
    let svc = ctx.get::<ByType>().await.expect("the injectable resolves");
    assert_eq!(svc.client.url, "shared");
    assert_eq!(DIAMOND_CONNECTS.load(Ordering::SeqCst), 1);
}
