//! Lifecycle hooks fire in construction order, and shutdown runs it backwards: a provider's
//! `on_module_init` runs after that of every provider it injects and its `on_module_destroy`
//! before, a module that declares each provider after what it injects fires them in declaration
//! order, sibling imports that import nothing fire in the order they are declared, controllers
//! fire after every provider, and a provider added beside them moves none of that.

use std::sync::Arc;
use std::sync::Mutex;

use serial_test::serial;
use ulo::{UloFactory, controller, injectable, module, on_module_destroy, on_module_init};

static LOG: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn log(event: &str) {
    LOG.lock().unwrap().push(event.to_string());
}

macro_rules! hooked {
    ($name:ident $(, $dep:ident: $dep_ty:ident)*) => {
        #[injectable]
        pub struct $name {
            $(
                #[inject]
                $dep: std::sync::Arc<$dep_ty>,
            )*
        }

        impl $name {
            #[on_module_init]
            async fn init(&self) -> ulo::di::InitResult {
                $( let _ = &self.$dep; )*
                log(concat!("init:", stringify!($name)));
                Ok(())
            }

            #[on_module_destroy]
            async fn destroy(&self) {
                log(concat!("destroy:", stringify!($name)));
            }
        }
    };
}

hooked!(Leaf);
hooked!(Mid, leaf: Leaf);
hooked!(Aardvark);
hooked!(Zebra);
hooked!(First);
hooked!(Second);
hooked!(Imported);
hooked!(Importer, imported: Imported);
hooked!(Left);
hooked!(Right);
hooked!(GlobalDep);

/// Runs `module` through startup and shutdown, answering the hooks in the order they fired.
async fn hooks_of(module: impl ulo::di::ModuleMetadata + 'static) -> Vec<String> {
    LOG.lock().unwrap().clear();
    let mut ctx = UloFactory::create_application_context(module)
        .await
        .expect("the module starts");
    ctx.close().await;
    LOG.lock().unwrap().clone()
}

#[module(providers: [Mid, Leaf])]
struct DependentFirst;

#[module(providers: [Mid, Aardvark, Leaf])]
struct WithAardvark;

#[module(providers: [Mid, Zebra, Leaf])]
struct WithZebra;

#[serial]
#[tokio::test]
async fn a_provider_starts_after_and_stops_before_what_it_injects() {
    assert_eq!(
        hooks_of(DependentFirst).await,
        ["init:Leaf", "init:Mid", "destroy:Mid", "destroy:Leaf"]
    );
}

// Aardvark sorts before Leaf and Mid and Zebra after, so building in token order fails one of them.
#[serial]
#[tokio::test]
async fn an_unrelated_provider_moves_no_hook() {
    assert_eq!(
        hooks_of(WithAardvark).await,
        [
            "init:Leaf",
            "init:Mid",
            "init:Aardvark",
            "destroy:Aardvark",
            "destroy:Mid",
            "destroy:Leaf",
        ]
    );
    assert_eq!(
        hooks_of(WithZebra).await,
        [
            "init:Leaf",
            "init:Mid",
            "init:Zebra",
            "destroy:Zebra",
            "destroy:Mid",
            "destroy:Leaf",
        ]
    );
}

#[module(providers: [Second, First])]
struct Independent;

#[serial]
#[tokio::test]
async fn independent_providers_keep_their_declaration_order() {
    assert_eq!(
        hooks_of(Independent).await,
        [
            "init:Second",
            "init:First",
            "destroy:First",
            "destroy:Second"
        ]
    );
}

#[module(providers: [Imported], exports: [Imported])]
struct ImportedModule;

#[module(imports: [ImportedModule], providers: [Importer])]
struct ImportingModule;

#[serial]
#[tokio::test]
async fn an_imported_module_starts_first_and_stops_last() {
    assert_eq!(
        hooks_of(ImportingModule).await,
        [
            "init:Imported",
            "init:Importer",
            "destroy:Importer",
            "destroy:Imported",
        ]
    );
}

#[module(providers: [Left])]
struct LeftModule;

#[module(providers: [Right])]
struct RightModule;

#[module(imports: [LeftModule, RightModule])]
struct Siblings;

#[serial]
#[tokio::test]
async fn sibling_imports_that_import_nothing_fire_in_declaration_order() {
    assert_eq!(
        hooks_of(Siblings).await,
        ["init:Left", "init:Right", "destroy:Right", "destroy:Left"]
    );
}

#[controller("/uses-global")]
pub struct UsesGlobal {
    #[inject]
    dep: Arc<GlobalDep>,
}

impl UsesGlobal {
    #[on_module_init]
    async fn init(&self) -> ulo::di::InitResult {
        let _ = &self.dep;
        log("init:UsesGlobal");
        Ok(())
    }

    #[on_module_destroy]
    async fn destroy(&self) {
        log("destroy:UsesGlobal");
    }
}

#[module(controllers: [UsesGlobal])]
struct ControllerModule;

/// Importing the controller's module builds it before this one, whose export the controller
/// injects; controllers are built after every provider, and their hooks follow.
#[module(imports: [ControllerModule], providers: [GlobalDep], exports: [GlobalDep], global: true)]
struct GlobalDepModule;

#[serial]
#[tokio::test]
async fn a_controller_starts_after_every_provider_and_stops_before() {
    assert_eq!(
        hooks_of(GlobalDepModule).await,
        [
            "init:GlobalDep",
            "init:UsesGlobal",
            "destroy:UsesGlobal",
            "destroy:GlobalDep",
        ]
    );
}
