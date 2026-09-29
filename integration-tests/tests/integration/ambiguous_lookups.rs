//! `.or_global()` reaches what the global modules export and no other module's provider, and a key
//! with two answers is refused rather than answered by whichever module the search reached first.

use ulo::di::{ModuleRef, ResolutionError};
use ulo::{UloFactory, injectable, module};

#[injectable]
pub struct Shared {
    #[default("global".to_string())]
    origin: String,
}

#[injectable]
pub struct Secret {}

#[injectable]
pub struct Looker {
    #[inject]
    module_ref: ModuleRef,
}

#[module(providers: [Shared], exports: [Shared], global: true)]
struct GlobalModule;

/// Exports nothing and is not global: `Secret` is its own.
#[module(providers: [Secret])]
struct PrivateModule;

#[module(providers: [Looker], exports: [Looker])]
struct LookerModule;

#[module(imports: [GlobalModule, PrivateModule, LookerModule])]
struct RootModule;

#[tokio::test]
async fn or_global_reaches_a_global_export_and_no_private_provider() {
    let ctx = UloFactory::create_application_context(RootModule)
        .await
        .expect("the module starts");
    let looker = ctx.get::<Looker>().await.unwrap();

    let shared = looker.module_ref.get::<Shared>().or_global().await;
    assert_eq!(shared.expect("a global export is reached").origin, "global");
    assert!(
        looker.module_ref.get::<Shared>().await.is_err(),
        "the current module alone does not hold it"
    );

    match looker.module_ref.get::<Secret>().or_global().await {
        Err(ResolutionError::ProviderNotFound { .. }) => {}
        Ok(_) => panic!("a provider another module kept private is not reached"),
        Err(other) => panic!("unexpected error: {other}"),
    }
}

#[injectable]
pub struct Port {
    #[default(0u16)]
    value: u16,
}

#[module(providers: [Port], exports: [Port])]
struct FirstPorts;

#[module(providers: [Port], exports: [Port])]
struct SecondPorts;

#[injectable]
pub struct Dialer {
    #[inject]
    port: Port,
}

#[module(imports: [FirstPorts, SecondPorts], providers: [Dialer])]
struct DialsAmbiguously;

#[module(imports: [FirstPorts, SecondPorts])]
struct ImportsBoth;

#[tokio::test]
async fn a_key_two_imports_export_is_refused_where_it_is_injected() {
    let err = match UloFactory::create_application_context(DialsAmbiguously).await {
        Ok(_) => panic!("an injection site with two answers must be refused"),
        Err(err) => err.to_string(),
    };
    assert!(err.contains("by two of its imports"), "{err}");
    assert!(
        err.contains("FirstPorts") && err.contains("SecondPorts"),
        "{err}"
    );

    UloFactory::create_application_context(ImportsBoth)
        .await
        .expect("two imports exporting one key start while nothing asks for it");
}

#[tokio::test]
async fn a_search_across_modules_hands_back_both_holders() {
    let ctx = UloFactory::create_application_context(ImportsBoth)
        .await
        .unwrap();

    let candidates = match ctx.get::<Port>().await {
        Err(ResolutionError::AmbiguousModule { candidates, .. }) => candidates,
        Ok(_) => panic!("two modules holding the key must not pick one"),
        Err(other) => panic!("unexpected error: {other}"),
    };
    assert_eq!(candidates.len(), 2, "{candidates:?}");
    for candidate in candidates {
        let module = ctx.get_module_by_id(&candidate).await.unwrap();
        let port = module.get::<Port>().await.expect("each holder resolves");
        assert_eq!(port.value, 0);
    }

    let single = UloFactory::create_application_context(FirstPorts)
        .await
        .unwrap();
    single.get::<Port>().await.expect("one holder answers");
}
