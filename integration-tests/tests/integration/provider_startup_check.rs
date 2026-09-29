//! A provider registered on a `DynamicModule` reports a failed startup check as a returned error
//! naming its module, and the check is part of the module's identity.
//!
//! This is the mechanism the database modules use to report a connection they could not establish:
//! `build` carries the failure into the provider, and `on_module_init`, which the scanner calls for
//! every non-execution-scoped provider, returns it beside the startup check's.

use std::any::Any;
use std::sync::Arc;

use ulo::di::{Execution, ResolutionError};

use ulo::di::{CheckedModule, DynamicModule, InitResult, ModuleMetadata};
use ulo::spi::{BuildResult, Provider, ProviderFactory, Registration};
use ulo::{FxHashMap, StartupCheck, StartupError, UloFactory, async_trait, module};
const TOKEN: &str = "PROBE_CONNECTION";

struct ProbeFactory {
    reachable: bool,
}

#[async_trait]
impl ProviderFactory for ProbeFactory {
    fn token(&self) -> String {
        TOKEN.to_string()
    }

    async fn build(&self, _deps: FxHashMap<String, Registration>) -> BuildResult<Registration> {
        // A real module would attempt its connection here and keep the Result.
        Ok(Registration::new(
            Arc::new(Box::new(ProbeProvider {
                reachable: self.reachable,
            })),
            vec![],
        ))
    }
}

struct ProbeProvider {
    reachable: bool,
}

#[async_trait]
impl Provider for ProbeProvider {
    fn token(&self) -> String {
        TOKEN.to_string()
    }

    async fn resolve(&self, _ctx: Execution) -> Result<Box<dyn Any + Send>, ResolutionError> {
        Ok(Box::new(()))
    }

    async fn on_module_init(&self) -> InitResult {
        if self.reachable {
            Ok(())
        } else {
            Err("connection refused".into())
        }
    }
}

fn probe_module(reachable: bool) -> DynamicModule {
    DynamicModule::builder("ProbeModule")
        .provider(ProbeFactory { reachable })
        .build()
}

#[tokio::test]
async fn a_dynamic_module_provider_reports_a_failed_startup_check() {
    let err = UloFactory::create_application_context(probe_module(false))
        .await
        .err()
        .expect("an unreachable connection must fail startup");

    assert!(
        matches!(&err, StartupError::HookFailed { hook, .. } if *hook == "on_module_init"),
        "expected a HookFailed from on_module_init, got: {err}"
    );
    let StartupError::HookFailed { module, .. } = &err else {
        unreachable!()
    };
    assert!(
        module.contains("ProbeModule"),
        "the failure should name the module, got: {module}"
    );
    assert!(
        err.to_string().contains("connection refused"),
        "the failure should carry the underlying cause, got: {err}"
    );
}

#[tokio::test]
async fn a_reachable_connection_starts_normally() {
    UloFactory::create_application_context(probe_module(true))
        .await
        .map(|_| ())
        .expect("a reachable connection must start");
}

/// What the probe connection is registered under.
pub struct ProbeConnection;

/// A connection factory whose identity hint is its connection string, as a database
/// integration's is.
struct ConnectionFactory;

#[async_trait]
impl ProviderFactory for ConnectionFactory {
    fn token(&self) -> String {
        ulo::di::token_of::<ProbeConnection>()
    }

    fn identity_hint(&self) -> Option<String> {
        Some("probe://db".to_string())
    }

    async fn build(&self, _deps: FxHashMap<String, Registration>) -> BuildResult<Registration> {
        Ok(Registration::new(
            Arc::new(Box::new(ConnectionProvider)),
            vec![],
        ))
    }
}

struct ConnectionProvider;

#[async_trait]
impl Provider for ConnectionProvider {
    fn token(&self) -> String {
        ulo::di::token_of::<ProbeConnection>()
    }

    async fn resolve(&self, _ctx: Execution) -> Result<Box<dyn Any + Send>, ResolutionError> {
        Ok(Box::new(()))
    }
}

/// A global module exporting the connection, built as a database integration's `for_root` is.
fn checked_connection() -> CheckedModule {
    CheckedModule::new(|_check| {
        DynamicModule::builder("ProbeConnectionModule")
            .provider(ConnectionFactory)
            .export::<ProbeConnection>()
            .global()
            .build()
    })
}

#[test]
fn a_startup_check_is_part_of_the_module_identity() {
    assert_eq!(
        checked_connection().identity(),
        checked_connection().identity()
    );
    assert_ne!(
        checked_connection().identity(),
        checked_connection().without_startup_check().identity()
    );
    assert_ne!(
        checked_connection().identity(),
        checked_connection()
            .with_startup_check(StartupCheck::default().attempts(1))
            .identity()
    );
}

#[module(imports: [checked_connection(), checked_connection()])]
struct IdenticalImports;

#[module(imports: [checked_connection(), checked_connection().without_startup_check()])]
struct ImportsWhoseChecksDiffer;

#[tokio::test]
async fn imports_differing_only_in_their_check_are_refused_where_they_clash() {
    UloFactory::create_application_context(IdenticalImports)
        .await
        .expect("identical imports dedup");

    let err = match UloFactory::create_application_context(ImportsWhoseChecksDiffer).await {
        Err(err) => err.to_string(),
        Ok(_) => panic!("a checked and an unchecked import of one connection must not pick one"),
    };
    assert!(err.contains("exported globally by two modules"), "{err}");
}
