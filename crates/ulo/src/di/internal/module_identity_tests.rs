//! Module-identity behavior: dynamic modules key on base name + config fingerprint, so identical
//! registrations dedup (a diamond import) while different configs stay distinct; two global modules
//! exporting the same token are refused instead of silently shadowing; a name gives each instance a
//! distinct token so deliberate multiplicity resolves.

use parking_lot::RwLock;
use std::any::Any;
use std::sync::Arc;

use async_trait::async_trait;

use crate::FxHashMap;
use crate::di::DynamicModule;
use crate::di::ResolutionError;
use crate::di::internal::scanner::DependencyScanner;
use crate::di::internal::{Container, InstanceLoader};
use crate::di::{Execution, ModuleMetadata};
use crate::dispatch::ControllerFactory;
use crate::spi::{BuildResult, Provider, ProviderFactory, Registration};
/// A provider that builds a trivial value. `token` is its injection token; `hint` is the
/// configuration fingerprint folded into the owning module's identity.
struct FakeFactory {
    token: String,
    hint: Option<String>,
}

#[async_trait]
impl ProviderFactory for FakeFactory {
    fn token(&self) -> String {
        self.token.clone()
    }

    fn identity_hint(&self) -> Option<String> {
        self.hint.clone()
    }

    async fn build(&self, _deps: FxHashMap<String, Registration>) -> BuildResult<Registration> {
        Ok(Registration::new(
            Arc::new(FakeProvider {
                token: self.token.clone(),
            }),
            vec![],
        ))
    }
}

struct FakeProvider {
    token: String,
}

#[async_trait]
impl Provider for FakeProvider {
    fn token(&self) -> String {
        self.token.clone()
    }
    async fn resolve(&self, _ctx: Execution) -> Result<Box<dyn Any + Send>, ResolutionError> {
        Ok(Box::new(0i32))
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

#[async_trait]
impl ModuleMetadata for Root {
    fn identity(&self) -> crate::di::ModuleIdentity {
        crate::di::ModuleIdentity::named("test::Root")
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

struct Conn;
struct Primary;
struct Replica;

/// A global module exporting one connection under `K`'s slot, fingerprinted by `url`.
fn conn_module<K: 'static>(base: &str, url: &str) -> DynamicModule {
    DynamicModule::builder(base)
        .provider(FakeFactory {
            token: crate::di::token_of::<K>(),
            hint: Some(url.into()),
        })
        .export::<K>()
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
    assert_eq!(m.identity().key(), "Mod");
}

#[test]
fn same_config_shares_identity_but_different_config_splits_it() {
    let a = conn_module::<Conn>("Conn", "postgres://a");
    let a_again = conn_module::<Conn>("Conn", "postgres://a");
    let b = conn_module::<Conn>("Conn", "postgres://b");

    // Same base + same config → same identity (a diamond import dedups).
    assert_eq!(a.identity().key(), a_again.identity().key());
    // Same base + different config → distinct identities (both survive to the clash check).
    assert_ne!(a.identity().key(), b.identity().key());
    // The base survives in the key, ahead of the fingerprint.
    assert_eq!(a.identity().base(), "Conn");
    assert!(a.identity().key().starts_with("Conn#"));
}

// ── Mechanism 4: add_module dedups on identity ───────────────────────────────────────────────

#[test]
fn add_module_dedups_identical_dynamic_modules() {
    let mut container = Container::new();
    container
        .add_module(Box::new(conn_module::<Conn>("Conn", "postgres://a")))
        .unwrap();
    container
        .add_module(Box::new(conn_module::<Conn>("Conn", "postgres://a")))
        .unwrap();
    assert_eq!(container.module_tokens().len(), 1);
}

// ── Mechanism 3 + the knob: end-to-end through scanner + loader ───────────────────────────────

async fn load(root: Root) -> crate::error::SetupResult<Arc<RwLock<Container>>> {
    let container = Arc::new(RwLock::new(Container::new()));
    let mut scanner = DependencyScanner::new(container.clone());
    scanner.scan(Box::new(crate::di::internal::builtin_module::BuiltinModule))?;
    scanner.scan(Box::new(root))?;
    scanner.scan_middleware()?;
    InstanceLoader::new(container.clone())
        .create_instances_of_dependencies()
        .await?;
    Ok(container)
}

#[tokio::test]
async fn two_unnamed_connections_are_refused() {
    let root = Root::new(vec![
        Box::new(conn_module::<Conn>("Conn", "postgres://a")),
        Box::new(conn_module::<Conn>("Conn", "postgres://b")),
    ]);
    let msg = match load(root).await {
        Ok(_) => panic!("clash must abort startup"),
        Err(e) => e.to_string(),
    };
    assert!(
        msg.contains("exported globally by two modules")
            && msg.contains(&crate::di::token_of::<Conn>()),
        "unexpected error: {msg}"
    );
}

#[tokio::test]
async fn named_connections_coexist_and_resolve() {
    let root = Root::new(vec![
        Box::new(conn_module::<Primary>("Conn::primary", "postgres://a")),
        Box::new(conn_module::<Replica>("Conn::replica", "postgres://b")),
    ]);
    let container = load(root).await.expect("named connections must coexist");
    let c = container.read();
    assert!(
        c.get_global_provider(&crate::di::token_of::<Primary>())
            .is_some()
    );
    assert!(
        c.get_global_provider(&crate::di::token_of::<Replica>())
            .is_some()
    );
}

#[tokio::test]
async fn same_connection_imported_twice_dedups() {
    // Two identical registrations (same base, same config) — a diamond import. They collapse
    // to one module, so there is no clash and the single connection resolves.
    let root = Root::new(vec![
        Box::new(conn_module::<Conn>("Conn", "postgres://a")),
        Box::new(conn_module::<Conn>("Conn", "postgres://a")),
    ]);
    let container = load(root)
        .await
        .expect("identical imports must dedup, not clash");
    assert!(
        container
            .read()
            .get_global_provider(&crate::di::token_of::<Conn>())
            .is_some()
    );
}
