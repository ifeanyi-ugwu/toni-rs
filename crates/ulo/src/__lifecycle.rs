//! Bridge between a `#[injectable]` provider and its optional lifecycle-hook methods.
//!
//! Lifecycle hooks (`#[on_module_init]`, `#[on_application_bootstrap]`, `#[on_module_destroy]`, `#[before_application_shutdown]`,
//! `#[on_application_shutdown]`) live on the user's `impl`, which the derive can't see. Each hook macro emits an
//! inherent `__ulo_lc_*` method forwarding to the user's; the derive's generated `Provider` impl
//! always calls `self.instance.__ulo_lc_*(..)`. Method resolution picks the inherent hook when
//! present, else the blanket no-op default below — so the derive needs no knowledge of which hooks
//! exist (it dispatches, it doesn't detect).
//!
//! Hooks are uniformly `async`, matching the `Provider` trait. The inherent calls must sit at a
//! concrete-type site (the generated wrapper names the struct); the inherent-wins resolution is a
//! property of that site.

#![doc(hidden)]

use async_trait::async_trait;

use crate::di::InitResult;

/// Blanket no-op lifecycle defaults, implemented for every type. The `#[on_*]` hook macros shadow
/// the relevant method with an inherent `async fn` of the same name, which wins at the call site.
#[async_trait]
pub trait LifecycleBridge {
    async fn __ulo_lc_on_init(&self) -> InitResult {
        Ok(())
    }
    async fn __ulo_lc_on_bootstrap(&self) -> InitResult {
        Ok(())
    }
    async fn __ulo_lc_on_destroy(&self) {}
    async fn __ulo_lc_before_shutdown(&self, _signal: Option<String>) {}
    async fn __ulo_lc_on_shutdown(&self, _signal: Option<String>) {}
}

impl<T: ?Sized + Sync> LifecycleBridge for T {}

/// Blanket `false` for each hook, shadowed by the inherent `true` its `#[on_*]` macro emits.
/// `#[injectable]` reads the five in `const` items to refuse a hook its scope never fires. Apart
/// from [`LifecycleBridge`] because they carry no `Sync` bound: the check reads them for every
/// execution-scoped and transient type, hooked or not.
pub trait LifecycleFlags {
    const __ULO_LC_ON_INIT: bool = false;
    const __ULO_LC_ON_BOOTSTRAP: bool = false;
    const __ULO_LC_ON_DESTROY: bool = false;
    const __ULO_LC_BEFORE_SHUTDOWN: bool = false;
    const __ULO_LC_ON_SHUTDOWN: bool = false;
}

impl<T: ?Sized> LifecycleFlags for T {}
