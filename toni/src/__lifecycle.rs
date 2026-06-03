//! Bridge between a `#[derive(Injectable)]` provider and its optional lifecycle-hook methods.
//!
//! Lifecycle hooks (`#[on_init]`, `#[on_bootstrap]`, `#[on_destroy]`, `#[before_shutdown]`,
//! `#[on_shutdown]`) live on the user's `impl`, which the derive can't see. Each hook macro emits an
//! inherent `__toni_lc_*` method forwarding to the user's; the derive's generated `Provider` impl
//! always calls `self.instance.__toni_lc_*(..)`. Method resolution picks the inherent hook when
//! present, else the blanket no-op default below — so the derive needs no knowledge of which hooks
//! exist (it dispatches, it doesn't detect).
//!
//! Hooks are uniformly `async`, matching the `Provider` trait. The inherent calls must sit at a
//! concrete-type site (the generated wrapper names the struct); the inherent-wins resolution is a
//! property of that site.

#![doc(hidden)]

use async_trait::async_trait;

use crate::InitResult;

/// Blanket no-op lifecycle defaults, implemented for every type. The `#[on_*]` hook macros shadow
/// the relevant method with an inherent `async fn` of the same name, which wins at the call site.
#[async_trait]
pub trait LifecycleBridge {
    async fn __toni_lc_on_init(&self) -> InitResult {
        Ok(())
    }
    async fn __toni_lc_on_bootstrap(&self) -> InitResult {
        Ok(())
    }
    async fn __toni_lc_on_destroy(&self) {}
    async fn __toni_lc_before_shutdown(&self, _signal: Option<String>) {}
    async fn __toni_lc_on_shutdown(&self, _signal: Option<String>) {}
}

impl<T: ?Sized + Sync> LifecycleBridge for T {}
