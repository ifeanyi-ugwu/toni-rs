//! P15b: the same `Timer` with `sleep` written `-> impl Future<Output = ()> + Send`. Expected:
//! fails, E0038 — `dyn Timer` cannot be formed, so no `Dep<dyn Timer>` binding can exist.
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

pub trait Timer: Send + Sync + 'static {
    fn sleep(&self, d: Duration) -> impl Future<Output = ()> + Send;
}
pub struct InstantTimer;
impl Timer for InstantTimer { async fn sleep(&self, _d: Duration) {} }

fn main() {
    let _bound: Arc<dyn Timer> = Arc::new(InstantTimer);
}
