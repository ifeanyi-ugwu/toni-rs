//! `RpcClientModule`: binds an `RpcClient` over one link (transports DESIGN §5.4).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use ulo::{Bound, BoxError, Dep, Module, ModuleDef, ModuleIdentity, Runtime};

use crate::client::RpcClient;
use crate::link::Link;

/// Binds `RpcClient` over link `L`, exported, usually keyed so two clients coexist:
///
/// ```ignore
/// #[module(imports = [RpcClientModule::for_root(ulo_rpc_nats::Nats::url("nats://bus:4222"))
///     .timeout(Bound::After(Duration::from_secs(2)))
///     .keyed::<Billing>()])]
/// pub struct OrdersModule;
/// ```
///
/// The link connects lazily, on the client's first call, so `connect` does no network I/O for it.
/// The client spawns its tasks on the app's runtime, which also times its calls, reading it as
/// `Dep<dyn Runtime>`: an app without one, `.timer(..)` alone included, fails `wire()` naming the
/// missing binding. The binding's `on_destroy` hook calls the
/// link's `close`, so the client's connection ends with the app rather than when the runtime
/// drops it; a call made after that connects again.
///
/// Each `for_root` is its own module: two clients of one link type are two imports, never one
/// deduplicated, and diagnostics name either `RpcClientModule`.
pub struct RpcClientModule<L: Link> {
    pub(crate) link: Arc<L>,
    pub(crate) timeout: Bound,
    pub(crate) instance: u64,
}

/// What tells two `for_root` calls apart.
#[derive(Clone, PartialEq, Eq, Hash)]
struct Instance(u64);

static INSTANCES: AtomicU64 = AtomicU64::new(0);

impl<L: Link> RpcClientModule<L> {
    pub fn for_root(link: L) -> Self {
        RpcClientModule { link: Arc::new(link), timeout: Bound::Default, instance: INSTANCES.fetch_add(1, Ordering::Relaxed) }
    }

    /// Every call's timeout unless the call sets its own: five seconds at `Bound::Default`,
    /// `Bound::Unbounded` for none. `Bound::After(Duration::ZERO)` is refused when the app wires.
    pub fn timeout(mut self, timeout: Bound) -> Self {
        self.timeout = timeout;
        self
    }
}

impl<L: Link> Module for RpcClientModule<L> {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_value(&Instance(self.instance)).label("RpcClientModule")
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        if self.timeout == Bound::After(Duration::ZERO) {
            // A module has no `prepare`; the refusal is recorded for `wire()` to report beside every
            // other wiring error.
            let refusal: Result<RpcClient, BoxError> = Err(format!(
                "`RpcClientModule::timeout(Bound::After(Duration::ZERO))` on the {} link would time out every call; \
                 write `Bound::Unbounded` to turn the timeout off",
                L::NAME
            )
            .into());
            m.try_value(refusal);
        } else {
            let link = Arc::clone(&self.link);
            let closing = Arc::clone(&self.link);
            let timeout = self.timeout;
            m.singleton(move |runtime: Dep<dyn Runtime>| {
                let link = Arc::clone(&link);
                async move { RpcClient::of_module(link, timeout, runtime.into_arc()) }
            })
            .on_destroy(move || {
                let link = Arc::clone(&closing);
                async move {
                    if let Err(error) = link.close().await {
                        tracing::warn!(%error, link = L::NAME, "the RPC client's link did not close cleanly");
                    }
                }
            });
        }
        m.export::<RpcClient>();
    }
}
