//! Bridge between a `#[injectable]` provider and an optional `#[new]` constructor.
//!
//! The struct macro generates the provider factory from the struct's fields and cannot see a
//! `#[new]` method on a separate `impl`. Rather than *detect* the constructor, the struct macro
//! reads `<Struct>::__ULO_ONE_NEW_PER_TYPE` at a site where the type is concrete. Path resolution
//! does the dispatch: `#[new]` emits an inherent associated const of that name, which out-ranks the
//! blanket [`CtorBridge`] default below, so a type with a constructor reads `Some(..)` (build via
//! the constructor) and any other type reads `None` (fall back to field injection). The struct
//! macros read the same const in a `const` item to refuse a `#[default]` field the constructor
//! overrides.
//!
//! The const is the only fixed-name item `#[new]` emits, so a second `#[new]` on one type fails as
//! a single duplicate definition of it, labelled at both attributes.
//!
//! The factory must read it at a concrete-type site (the generated code names the struct); the
//! inherent-wins resolution is a property of the site, not available through a generic `T`.

#![doc(hidden)]

use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::Arc;

use rustc_hash::FxHashMap;

use crate::di::Execution;
use crate::spi::Provider;

/// The already-built dependency providers passed to a factory's `build`, keyed by token.
pub type ResolvedDeps = FxHashMap<String, Arc<Box<dyn Provider>>>;

/// A `#[new]` constructor: `tokens` returns its dependency tokens (so the factory can declare
/// them), and `build` resolves those dependencies and calls it.
///
/// The context parameter carries the execution being served, so a constructor parameter that is
/// itself execution-scoped resolves in that same execution; it is `Execution::None` for
/// construction outside any execution, matching the field-injection paths.
pub struct Ctor<T> {
    pub tokens: fn() -> Vec<String>,
    pub build:
        for<'a> fn(&'a ResolvedDeps, Execution) -> Pin<Box<dyn Future<Output = T> + Send + 'a>>,
}

/// The blanket "no constructor" default, implemented for every type. `#[new]` shadows it with an
/// inherent associated const of the same name; the struct macros read the name unqualified at a
/// concrete-type site, so the inherent const wins where it exists.
pub trait CtorBridge: Sized {
    const __ULO_ONE_NEW_PER_TYPE: Option<Ctor<Self>> = None;
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
