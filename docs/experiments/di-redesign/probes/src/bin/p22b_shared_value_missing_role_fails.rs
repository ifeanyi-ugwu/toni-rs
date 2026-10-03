//! P22b (X2): P22 with `RateLimit` implementing `Guard<Http>` alone. The impl-level value is
//! shared with the RPC handler too, and that handler's mount fn carries the `Guard<Rpc>` bound,
//! so the call in `mount` fails. Expected: E0277 "`RateLimit: Guard<Rpc>` is not satisfied" at the
//! `__fw_mount_get_rpc` call, which `#[routes]` spans at the handler's name.
#![allow(non_upper_case_globals)]
use std::sync::Arc;

pub trait Transport: 'static {}
pub struct Http;
pub struct Rpc;
impl Transport for Http {}
impl Transport for Rpc {}

pub trait Guard<T: Transport>: Send + Sync + 'static {}
pub type AnyGuard<T> = dyn Guard<T>;

pub struct RateLimit(pub u32);
impl Guard<Http> for RateLimit {}

pub struct Spec<T: Transport> {
    guards: Vec<Arc<AnyGuard<T>>>,
}
impl<T: Transport> Spec<T> {
    fn new() -> Self {
        Spec { guards: Vec::new() }
    }
    fn guard_arc<G: Guard<T>>(&mut self, guard: Arc<G>) {
        let widened: Arc<AnyGuard<T>> = guard;
        self.guards.push(widened);
    }
}

pub struct __FwShared<V0> {
    v0: Arc<V0>,
}

pub struct Ctl;

impl Ctl {
    fn __fw_mount_get<V0: Guard<Http>>(shared: &__FwShared<V0>) -> Spec<Http> {
        let mut spec = Spec::new();
        spec.guard_arc(Arc::clone(&shared.v0));
        spec
    }

    fn __fw_mount_get_rpc<V0: Guard<Rpc>>(shared: &__FwShared<V0>) -> Spec<Rpc> {
        let mut spec = Spec::new();
        spec.guard_arc(Arc::clone(&shared.v0));
        spec
    }

    fn mount() -> (Spec<Http>, Spec<Rpc>) {
        let shared = __FwShared { v0: Arc::new(RateLimit(20)) };
        (Self::__fw_mount_get(&shared), Self::__fw_mount_get_rpc(&shared))
    }
}

fn main() {
    let _ = Ctl::mount();
}
