use std::sync::Arc;

use async_trait::async_trait;
use rustc_hash::FxHashMap;

use crate::error::BuildResult;
use crate::http::Route;
use crate::spi::provider::Provider;

/// What a controller hands over to be dispatched on.
///
/// The one place the transports differ. HTTP dispatches on routes keyed by path and method, RPC on a
/// set of patterns, gRPC on a registration with the tonic router. Everything else a controller
/// carries — its token, its dependencies, its lifecycle, the scope it is built at — is common to all
/// three and lives on [`Controller`] itself.
pub enum Targets {
    Http(Vec<Arc<dyn Route>>),
    Rpc(Arc<dyn crate::rpc::RpcControllerSource>),
    Grpc(Arc<dyn crate::grpc::GrpcServiceSource>),
}

/// A controller: one DI instance exposing what it dispatches on and its lifecycle hooks.
///
/// Built once per controller struct by its [`ControllerFactory`]. [`targets`](Controller::targets)
/// yields the transport's own dispatch surface — one [`Route`] per handler method on HTTP, a single
/// source on RPC and gRPC. The lifecycle hooks fire once per controller, not once per route or
/// pattern.
#[async_trait]
pub trait Controller: Send + Sync {
    fn token(&self) -> String;
    fn targets(&self) -> Targets;

    // Lifecycle Hooks

    async fn on_module_init(&self) -> crate::di::InitResult {
        Ok(())
    }
    async fn on_application_bootstrap(&self) -> crate::di::InitResult {
        Ok(())
    }
    async fn before_application_shutdown(&self, _signal: Option<String>) {}
    async fn on_module_destroy(&self) {}
    async fn on_application_shutdown(&self, _signal: Option<String>) {}
}

#[async_trait]
pub trait ControllerFactory: Send + Sync {
    fn token(&self) -> String;
    fn dependency_tokens(&self) -> Vec<String> {
        vec![]
    }

    /// The dependencies a field or parameter written as a plain type reads, each with the type it
    /// is written as. The loader refuses the build when one of them hands out one shared instance,
    /// [`Shape::Shared`](crate::spi::Shape::Shared), which a consumer built per execution would
    /// otherwise meet at its first resolution.
    fn value_dependencies(&self) -> Vec<(String, &'static str)> {
        vec![]
    }

    async fn build(
        &self,
        deps: FxHashMap<String, Arc<dyn Provider>>,
    ) -> BuildResult<Arc<dyn Controller>>;
}
