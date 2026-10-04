//! `grpc.health.v1` over tonic-health (transports DESIGN §6.2).

use tonic::server::NamedService;
use tonic_health::ServingStatus;
use tonic_health::server::HealthReporter;

/// The health reporter, bound by the gRPC server as `Dep<GrpcHealth>`: SERVING for every known
/// service after `bind`, NOT_SERVING from the drain on, and in between what the application sets.
#[derive(Clone)]
pub struct GrpcHealth {
    pub(crate) reporter: HealthReporter,
}

impl GrpcHealth {
    /// Sets service `S`'s status.
    pub async fn set<S: NamedService>(&self, status: ServingStatus) {
        let _ = (status, &self.reporter);
        todo!()
    }

    /// Sets the status of the service named `name`, `users.v1.UserService`.
    pub async fn set_named(&self, name: &str, status: ServingStatus) {
        let _ = (name, status);
        todo!()
    }
}
