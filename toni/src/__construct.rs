//! Bridge between a `#[derive(Injectable)]` provider and an optional `#[new]` constructor.
//!
//! The derive generates the provider factory from the struct's fields and cannot see a `#[new]`
//! method on a separate `impl`. Rather than *detect* the constructor, the derive simply *calls* it:
//! the factory invokes `Self::__toni_ctor_build(deps)` and `Self::__toni_ctor_tokens()` at a site
//! where the type is concrete. Method resolution does the dispatch — `#[new]` emits inherent
//! associated fns that out-rank the blanket [`CtorBridge`] defaults below, so a type with a
//! constructor returns `Some(..)` (build via the constructor) and any other type returns `None`
//! (fall back to field injection).
//!
//! The factory must call these at a concrete-type site (the generated code names the struct); the
//! inherent-wins resolution is a property of the call site, not available through a generic `T`.

#![doc(hidden)]

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use rustc_hash::FxHashMap;

use crate::traits_helpers::Provider;

/// The already-built dependency providers passed to a factory's `build`, keyed by token.
pub type ResolvedDeps = FxHashMap<String, Arc<Box<dyn Provider>>>;

/// Blanket "no constructor" defaults, implemented for every type. `#[new]` shadows these with
/// inherent associated fns of the same name; the derive's factory calls the names unqualified at a
/// concrete-type site, so the inherent versions win where they exist.
///
/// `__toni_ctor_tokens` returns the constructor's dependency tokens (so the factory can declare
/// them); `__toni_ctor_build` resolves those dependencies and calls the constructor. `None` from
/// either means "no `#[new]` — use field injection".
pub trait CtorBridge: Sized {
    fn __toni_ctor_tokens() -> Option<Vec<String>> {
        None
    }

    fn __toni_ctor_build<'a>(
        _deps: &'a ResolvedDeps,
    ) -> Option<Pin<Box<dyn Future<Output = Self> + Send + 'a>>> {
        None
    }
}

impl<T> CtorBridge for T {}
