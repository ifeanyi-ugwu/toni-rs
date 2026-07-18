//! Module-identity behavior: dynamic modules key on base name + config fingerprint, so identical
//! registrations dedup (a diamond import) while different configs stay distinct; two global modules
//! exporting the same token are refused instead of silently shadowing; a name gives each instance a
//! distinct token so deliberate multiplicity resolves.

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use async_trait::async_trait;

use crate::DynamicModule;
use crate::FxHashMap;
use crate::injector::{ToniContainer, ToniInstanceLoader};
use crate::scanner::ToniDependenciesScanner;
use crate::traits_helpers::{
    ControllerFactory, Injectable, ModuleMetadata, Provider, ProviderContext, ProviderFactory,
};

/// A provider that builds a trivial value. `token` is its injection token; `hint` is the
/// configuration fingerprint folded into the owning module's identity.
struct FakeFactory {
    token: String,
    hint: Option<String>,
}

#[async_trait]
impl ProviderFactory for FakeFactory {
    fn get_token(&self) -> String {
        self.token.clone()
    }

    fn identity_hint(&self) -> Option<String> {
        self.hint.clone()
    }

    async fn build(&self, _deps: FxHashMap<String, Injectable>) -> Injectable {
        Injectable::new(
            Arc::new(Box::new(FakeProvider {
                token: self.token.clone(),
            })),
            vec![],
        )
    }
}

struct FakeProvider {
    token: String,
}

#[async_trait]
impl Provider for FakeProvider {
    fn get_token(&self) -> String {
        self.token.clone()
    }
    fn get_token_factory(&self) -> String {
        self.token.clone()
    }
    async fn execute(
        &self,
        _params: Vec<Box<dyn Any + Send>>,
        _ctx: ProviderContext<'_>,
    ) -> Box<dyn Any + Send> {
        Box::new(0i32)
    }
}

/// A non-global root that imports whatever modules the test supplies. `imports()` drains on first
/// call — the scanner calls it exactly once — so the modules are stored behind a take-once cell.
struct Root {
    imports: parking_lot::Mutex<Option<Vec<Box<dyn ModuleMetadata>>>>,
}

impl Root {
    fn new(imports: Vec<Box<dyn ModuleMetadata>>) -> Self {
        Self {
            imports: parking_lot::Mutex::new(Some(imports)),
        }
    }
}

#[async_trait(?Send)]
impl ModuleMetadata for Root {
    fn get_id(&self) -> String {
        "test::Root".into()
    }
    fn get_name(&self) -> String {
        "Root".into()
    }
    fn imports(&self) -> Option<Vec<Box<dyn ModuleMetadata>>> {
        self.imports.lock().take()
    }
    fn controllers(&self) -> Option<Vec<Box<dyn ControllerFactory>>> {
        None
    }
    fn providers(&self) -> Option<Vec<Box<dyn ProviderFactory>>> {
        None
    }
    fn exports(&self) -> Option<Vec<String>> {
        None
    }
}

/// A global module exporting one connection under `token`, fingerprinted by `url`.
fn conn_module(base: &str, token: &str, url: &str) -> DynamicModule {
    DynamicModule::builder(base)
        .provider(FakeFactory {
            token: token.into(),
            hint: Some(url.into()),
        })
        .export_token(token)
        .global()
        .build()
}

// ── Mechanism 2: dynamic identity folds in the config fingerprint ────────────────────────────

#[test]
fn no_hint_keeps_base_identity() {
    let m = DynamicModule::builder("Mod")
        .provider(FakeFactory {
            token: "t".into(),
            hint: None,
        })
        .build();
    assert_eq!(m.get_id(), "Mod");
    assert_eq!(m.get_name(), "Mod");
}

#[test]
fn same_config_shares_identity_but_different_config_splits_it() {
    let a = conn_module("Conn", "conn", "postgres://a");
    let a_again = conn_module("Conn", "conn", "postgres://a");
    let b = conn_module("Conn", "conn", "postgres://b");

    // Same base + same config → same identity (a diamond import dedups).
    assert_eq!(a.get_id(), a_again.get_id());
    // Same base + different config → distinct identities (both survive to the clash check).
    assert_ne!(a.get_id(), b.get_id());
    // Display name is the base regardless of fingerprint.
    assert_eq!(a.get_name(), "Conn");
    assert!(a.get_id().starts_with("Conn#"));
}

// ── Mechanism 4: add_module dedups on identity ───────────────────────────────────────────────

#[test]
fn add_module_dedups_identical_dynamic_modules() {
    let mut container = ToniContainer::new();
    container
        .add_module(Box::new(conn_module("Conn", "conn", "postgres://a")))
        .unwrap();
    container
        .add_module(Box::new(conn_module("Conn", "conn", "postgres://a")))
        .unwrap();
    assert_eq!(container.get_modules_token().len(), 1);
}

// ── Mechanism 3 + the knob: end-to-end through scanner + loader ───────────────────────────────

async fn load(root: Root) -> anyhow::Result<Rc<RefCell<ToniContainer>>> {
    let container = Rc::new(RefCell::new(ToniContainer::new()));
    let mut scanner = ToniDependenciesScanner::new(container.clone());
    scanner.scan(Box::new(crate::builtin_module::BuiltinModule))?;
    scanner.scan(Box::new(root))?;
    scanner.scan_middleware()?;
    ToniInstanceLoader::new(container.clone())
        .create_instances_of_dependencies()
        .await?;
    Ok(container)
}

#[tokio::test]
async fn two_unnamed_connections_are_refused() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let root = Root::new(vec![
                Box::new(conn_module("Conn", "conn", "postgres://a")),
                Box::new(conn_module("Conn", "conn", "postgres://b")),
            ]);
            let msg = match load(root).await {
                Ok(_) => panic!("clash must abort startup"),
                Err(e) => e.to_string(),
            };
            assert!(
                msg.contains("exported globally by two modules") && msg.contains("conn"),
                "unexpected error: {msg}"
            );
        })
        .await;
}

#[tokio::test]
async fn named_connections_coexist_and_resolve() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let root = Root::new(vec![
                Box::new(conn_module("Conn::primary", "primary", "postgres://a")),
                Box::new(conn_module("Conn::replica", "replica", "postgres://b")),
            ]);
            let container = load(root).await.expect("named connections must coexist");
            let c = container.borrow();
            assert!(c.get_global_provider(&"primary".to_string()).is_some());
            assert!(c.get_global_provider(&"replica".to_string()).is_some());
        })
        .await;
}

#[tokio::test]
async fn same_connection_imported_twice_dedups() {
    tokio::task::LocalSet::new()
        .run_until(async {
            // Two identical registrations (same base, same config) — a diamond import. They collapse
            // to one module, so there is no clash and the single connection resolves.
            let root = Root::new(vec![
                Box::new(conn_module("Conn", "conn", "postgres://a")),
                Box::new(conn_module("Conn", "conn", "postgres://a")),
            ]);
            let container = load(root)
                .await
                .expect("identical imports must dedup, not clash");
            assert!(
                container
                    .borrow()
                    .get_global_provider(&"conn".to_string())
                    .is_some()
            );
        })
        .await;
}
