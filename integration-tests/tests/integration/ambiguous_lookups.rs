//! `.visible()` reaches what the module could inject, an import's export ahead of a global
//! module's, and no provider exported to neither; a key with two answers is refused rather than
//! answered by whichever module the search reached first.

use ulo::di::{ModuleRef, ResolutionError};
use ulo::{UloFactory, injectable, key, module, provide};

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
async fn visible_reaches_a_global_export_and_no_private_provider() {
    let ctx = UloFactory::create_application_context(RootModule)
        .await
        .expect("the module starts");
    let looker = ctx.get::<Looker>().await.unwrap();

    let shared = looker.module_ref.get::<Shared>().visible().await;
    assert_eq!(shared.expect("a global export is reached").origin, "global");
    assert!(
        looker.module_ref.get::<Shared>().await.is_err(),
        "the current module alone does not hold it"
    );

    match looker.module_ref.get::<Secret>().visible().await {
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

#[injectable]
pub struct Svc {}

#[module(providers: [Svc], exports: [Svc])]
struct LibModule;

#[injectable]
pub struct Sibling {}

/// Exports `Sibling` to the root that imports it, not to `ImportsLib`.
#[module(providers: [Sibling], exports: [Sibling])]
struct SiblingModule;

#[injectable]
pub struct LibLooker {
    #[inject]
    module_ref: ModuleRef,
}

#[module(imports: [LibModule], providers: [LibLooker], exports: [LibLooker])]
struct ImportsLib;

#[module(imports: [ImportsLib, SiblingModule])]
struct LibRoot;

#[tokio::test]
async fn visible_reaches_an_export_its_module_imports() {
    let ctx = UloFactory::create_application_context(LibRoot)
        .await
        .expect("the module starts");
    let looker = ctx.get::<LibLooker>().await.unwrap();

    looker
        .module_ref
        .get::<Svc>()
        .visible()
        .await
        .expect("an import's export is reached");
    assert!(
        looker.module_ref.get::<Svc>().await.is_err(),
        "the current module alone does not hold it"
    );

    match looker.module_ref.get::<Sibling>().visible().await {
        Err(ResolutionError::ProviderNotFound { .. }) => {}
        Ok(_) => panic!("an export to another importer is not reached"),
        Err(other) => panic!("unexpected error: {other}"),
    }
}

#[injectable]
pub struct PortLooker {
    #[inject]
    module_ref: ModuleRef,
}

#[module(imports: [FirstPorts, SecondPorts], providers: [PortLooker])]
struct LooksAtBothPorts;

#[tokio::test]
async fn visible_refuses_a_key_two_imports_export() {
    let ctx = UloFactory::create_application_context(LooksAtBothPorts)
        .await
        .expect("nothing injects the key, so the module starts");
    let looker = ctx.get::<PortLooker>().await.unwrap();

    let candidates = match looker.module_ref.get::<Port>().visible().await {
        Err(ResolutionError::AmbiguousModule { candidates, .. }) => candidates,
        Ok(_) => panic!("two imports' exports must not pick one"),
        Err(other) => panic!("unexpected error: {other}"),
    };
    assert_eq!(candidates.len(), 2, "{candidates:?}");
    assert!(
        candidates.iter().any(|key| key.ends_with("::FirstPorts"))
            && candidates.iter().any(|key| key.ends_with("::SecondPorts")),
        "{candidates:?}"
    );
}

key!(pub Region: String);

#[module(providers: [provide!(Region => "global".to_string())], exports: [Region], global: true)]
struct GlobalRegion;

#[module(providers: [provide!(Region => "import".to_string())], exports: [Region])]
struct ImportedRegion;

#[module(providers: [provide!(Region => "second".to_string())], exports: [Region])]
struct SecondRegion;

#[injectable]
pub struct RegionLooker {
    #[inject]
    module_ref: ModuleRef,
}

#[module(imports: [ImportedRegion], providers: [RegionLooker], exports: [RegionLooker])]
struct OneRegionImport;

#[injectable]
pub struct TwoRegionLooker {
    #[inject]
    module_ref: ModuleRef,
}

#[module(
    imports: [ImportedRegion, SecondRegion],
    providers: [TwoRegionLooker],
    exports: [TwoRegionLooker],
)]
struct TwoRegionImports;

#[module(imports: [GlobalRegion, OneRegionImport, TwoRegionImports])]
struct RegionRoot;

#[tokio::test]
async fn visible_reads_an_import_before_the_global_registry() {
    let ctx = UloFactory::create_application_context(RegionRoot)
        .await
        .expect("the module starts");

    let looker = ctx.get::<RegionLooker>().await.unwrap();
    let region = looker.module_ref.get_key::<Region>().visible().await;
    assert_eq!(*region.expect("the import answers"), "import");

    let looker = ctx.get::<TwoRegionLooker>().await.unwrap();
    match looker.module_ref.get_key::<Region>().visible().await {
        Err(ResolutionError::AmbiguousModule { candidates, .. }) => {
            assert_eq!(candidates.len(), 2, "{candidates:?}")
        }
        Ok(value) => panic!("two imports must not pick one, nor defer to the global: {value}"),
        Err(other) => panic!("unexpected error: {other}"),
    }
}
