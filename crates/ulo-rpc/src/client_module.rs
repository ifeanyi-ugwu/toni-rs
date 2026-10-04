//! `RpcClientModule`: binds an `RpcClient` over one link (transports DESIGN §5.4).

use ulo::{Bound, Module, ModuleDef, ModuleIdentity};

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
pub struct RpcClientModule<L: Link> {
    pub(crate) link: L,
    pub(crate) timeout: Bound,
}

impl<L: Link> RpcClientModule<L> {
    pub fn for_root(link: L) -> Self {
        RpcClientModule { link, timeout: Bound::Default }
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
        todo!()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        let _ = m;
        todo!()
    }
}
