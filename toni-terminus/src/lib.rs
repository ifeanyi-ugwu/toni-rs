mod health_check_result;
mod health_check_service;
mod health_indicator;
mod module;

pub mod indicators;

pub use health_check_result::{HealthCheckResult, HealthEntry, HealthIndicatorResult};
pub use health_check_service::HealthCheckService;
pub use health_indicator::HealthIndicator;
pub use module::TerminusModule;

#[cfg(feature = "disk")]
pub use indicators::DiskHealthIndicator;
#[cfg(feature = "http")]
pub use indicators::HttpHealthIndicator;
#[cfg(feature = "memory")]
pub use indicators::MemoryHealthIndicator;
