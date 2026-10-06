//! `grpc.health.v1` over tonic-health (transports DESIGN §6.2).

use tonic::server::NamedService;
use tonic_health::ServingStatus;
use tonic_health::pb::health_server::HealthServer;
use tonic_health::server::{HealthReporter, HealthService};
use ulo::DynamicModule;

/// The health reporter, bound by the gRPC server as `Dep<GrpcHealth>`: SERVING for every known
/// service after `bind`, NOT_SERVING from the drain on, and in between what the application sets.
///
/// `imports = [GrpcHealth::module()]` binds it, global, for any module to read; the server finds
/// it there and serves the statuses it sets. Without the import the server keeps a reporter of its
/// own, and `grpc.health.v1` still answers.
#[derive(Clone)]
pub struct GrpcHealth {
    pub(crate) reporter: HealthReporter,
}

impl GrpcHealth {
    pub(crate) fn new() -> Self {
        GrpcHealth { reporter: HealthReporter::new() }
    }

    /// The global module binding one `GrpcHealth`, which the gRPC server serves through
    /// `grpc.health.v1`.
    pub fn module() -> DynamicModule {
        DynamicModule::new::<GrpcHealth, ()>((), |m| {
            m.value(GrpcHealth::new());
            m.export::<GrpcHealth>();
            m.global();
        })
        .label("GrpcHealth")
    }

    /// Sets service `S`'s status.
    pub async fn set<S: NamedService>(&self, status: ServingStatus) {
        self.reporter.set_service_status(S::NAME, status).await;
    }

    /// Sets the status of the service named `name`, `users.v1.UserService`.
    pub async fn set_named(&self, name: &str, status: ServingStatus) {
        self.reporter.set_service_status(name, status).await;
    }

    /// `grpc.health.v1.Health` answering this reporter's statuses.
    pub(crate) fn service(&self) -> HealthServer<HealthService> {
        HealthServer::new(HealthService::from_health_reporter(self.reporter.clone()))
    }
}
