//! A provider registered on a `DynamicModule` reports a failed startup check as a returned error
//! naming its module.
//!
//! This is the mechanism the database modules use to report a connection they could not establish,
//! without `ProviderFactory::build` needing to be fallible: `build` carries the failure into the
//! provider, and `on_module_init` — which the scanner calls for every non-request-scoped provider
//! — returns it.

use std::any::Any;
use std::sync::Arc;

use toni::traits_helpers::{Injectable, Provider, ProviderContext, ProviderFactory};
use toni::{async_trait, DynamicModule, FxHashMap, InitResult, StartupError, ToniFactory};

const TOKEN: &str = "PROBE_CONNECTION";

struct ProbeFactory {
    reachable: bool,
}

#[async_trait]
impl ProviderFactory for ProbeFactory {
    fn get_token(&self) -> String {
        TOKEN.to_string()
    }

    async fn build(&self, _deps: FxHashMap<String, Injectable>) -> Injectable {
        // A real module would attempt its connection here and keep the Result.
        Injectable::new(
            Arc::new(Box::new(ProbeProvider {
                reachable: self.reachable,
            })),
            vec![],
        )
    }
}

struct ProbeProvider {
    reachable: bool,
}

#[async_trait]
impl Provider for ProbeProvider {
    fn get_token(&self) -> String {
        TOKEN.to_string()
    }

    async fn execute(
        &self,
        _params: Vec<Box<dyn Any + Send>>,
        _ctx: ProviderContext,
    ) -> Box<dyn Any + Send> {
        Box::new(())
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
    let err = ToniFactory::create_application_context(probe_module(false))
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
    ToniFactory::create_application_context(probe_module(true))
        .await
        .map(|_| ())
        .expect("a reachable connection must start");
}
