//! Bridge between a `#[injectable]` provider and an optional `#[new]` constructor.
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
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::Arc;

use rustc_hash::FxHashMap;

use crate::traits_helpers::Provider;
use crate::traits_helpers::ProviderContext;

/// The already-built dependency providers passed to a factory's `build`, keyed by token.
pub type ResolvedDeps = FxHashMap<String, Arc<Box<dyn Provider>>>;

/// Blanket "no constructor" defaults, implemented for every type. `#[new]` shadows these with
/// inherent associated fns of the same name; the derive's factory calls the names unqualified at a
/// concrete-type site, so the inherent versions win where they exist.
///
/// `__toni_ctor_tokens` returns the constructor's dependency tokens (so the factory can declare
/// them); `__toni_ctor_build` resolves those dependencies and calls the constructor. `None` from
/// either means "no `#[new]` — use field injection".
///
/// `request_parts` carries the active HTTP request parts when one exists (request-scoped
/// construction), so a constructor parameter that is itself request-scoped can be resolved; it is
/// `None` for singleton/transient construction, matching the field-injection paths.
pub trait CtorBridge: Sized {
    fn __toni_ctor_tokens() -> Option<Vec<String>> {
        None
    }

    fn __toni_ctor_build<'a>(
        _deps: &'a ResolvedDeps,
        _ctx: ProviderContext,
    ) -> Option<Pin<Box<dyn Future<Output = Self> + Send + 'a>>> {
        None
    }
}

impl<T> CtorBridge for T {}

/// Probe for defaulting an owned (unmarked / `#[default]`-less) field.
///
/// The factory always emits a field-injection construction path, even for a provider that builds
/// itself through a `#[new]` constructor — the two macros can't see each other, so the path is
/// present but dead whenever a constructor exists. A direct `<FieldTy>::default()` there would force
/// every constructor-built field to implement `Default`, which the old constructor form never
/// required. Routing through this probe defers that requirement: a `Default` type still defaults,
/// and any other type compiles and only panics if the dead path is ever actually taken (no `#[new]`,
/// no `#[default(...)]`, no `Default`).
///
/// Resolution is autoref-specialization: the inherent `field_default` on `OwnedFieldDefault<T>` wins
/// for `T: Default` (zero autoref); otherwise the blanket [`OwnedFieldDefaultFallback`] on
/// `&OwnedFieldDefault<T>` is reached by one autoref.
pub struct OwnedFieldDefault<T>(pub PhantomData<T>);

impl<T> OwnedFieldDefault<T> {
    pub fn new() -> Self {
        OwnedFieldDefault(PhantomData)
    }
}

impl<T> Default for OwnedFieldDefault<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Default> OwnedFieldDefault<T> {
    pub fn field_default(&self, _field: &'static str, _ty: &'static str) -> T {
        T::default()
    }
}

pub trait OwnedFieldDefaultFallback {
    type Out;
    fn field_default(&self, field: &'static str, ty: &'static str) -> Self::Out;
}

impl<T> OwnedFieldDefaultFallback for &OwnedFieldDefault<T> {
    type Out = T;
    fn field_default(&self, field: &'static str, ty: &'static str) -> T {
        panic!(
            "owned field `{field}: {ty}` has no `Default` impl and no `#[default(...)]`. \
             Add `#[default(expr)]`, give `{ty}` a `Default` impl, or build the field in a \
             `#[new]` constructor."
        )
    }
}
