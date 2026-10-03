//! Scope markers and the bounds the compiler checks against them (§3.3).
//!
//! Scope is one axis whatever builds the binding: a type, a factory, or a closure. [`Auto`] is the
//! default everywhere, and [`Singleton`], [`PerExecution`] and [`Transient`] override it.
//!
//! What `Auto` means depends on the binding's role. For a provider it behaves exactly like
//! [`Singleton`], and depending on execution data is refused at `wire()`. For a controller or an
//! enhancer it resolves to singleton when nothing below it needs an execution, and to
//! per-execution otherwise. Writing `#[injectable(singleton)]` on a controller opts out of the
//! inference, so a controller declared that way that needs execution data is refused.
//!
//! A closure follows the same rule: [`Contribute::with`](crate::Contribute::with) and
//! [`EnhancerSpec::guard_with`](crate::EnhancerSpec::guard_with) declare `Auto`. Inference reads
//! what a closure's parameters read, not what its body does, so a closure that creates per-call
//! state from nothing per call, such as a timer started when it is built, is inferred built once.
//! That closure declares [`PerExecution`] explicitly.

/// Built once during `connect` and shared across the application.
pub struct Singleton;

/// Built once per execution and shared inside it.
pub struct PerExecution;

/// Built fresh for every injection point that reads it.
pub struct Transient;

/// What `#[injectable]` means when no scope is written, and what a closure declares when no
/// scope is written.
pub struct Auto;

mod sealed {
    pub trait Sealed {}
    impl Sealed for super::Singleton {}
    impl Sealed for super::PerExecution {}
    impl Sealed for super::Transient {}
    impl Sealed for super::Auto {}
}

/// The declared scope of a [`Construct`](crate::Construct) type or a factory binding.
pub trait Scope: sealed::Sealed + 'static {
    const KIND: ScopeKind;
}

/// A scope as a runtime value, carried by the binding record and printed in diagnostics.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ScopeKind {
    Singleton,
    PerExecution,
    Transient,
    Auto,
}

impl Scope for Singleton {
    const KIND: ScopeKind = ScopeKind::Singleton;
}

impl Scope for PerExecution {
    const KIND: ScopeKind = ScopeKind::PerExecution;
}

impl Scope for Transient {
    const KIND: ScopeKind = ScopeKind::Transient;
}

impl Scope for Auto {
    const KIND: ScopeKind = ScopeKind::Auto;
}

/// The scopes a lifecycle hook may be declared on. A hook on an explicitly execution-scoped or
/// transient type fails to compile; a hook on an `Auto` binding that the wiring pass infers
/// per-execution is refused at `wire()` (§6.2).
#[diagnostic::on_unimplemented(
    message = "lifecycle hooks are only allowed on singletons",
    label = "`{Self}` scope cannot have hooks",
    note = "remove the scope argument from #[injectable], or move the hook to a singleton"
)]
pub trait HookCapable: Scope {}

impl HookCapable for Singleton {}
impl HookCapable for Auto {}

/// The scopes an enhancer declared by closure names when it overrides `Auto`:
/// [`EnhancerSpec::guard_with_in`](crate::EnhancerSpec::guard_with_in) and its siblings take one.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not an explicit scope",
    label = "name `Singleton`, `PerExecution` or `Transient` here",
    note = "`Auto` is the default: write `guard_with`, `interceptor_with` or `error_handler_with` without a scope"
)]
pub trait ExplicitScope: Scope {}

impl ExplicitScope for Singleton {}
impl ExplicitScope for PerExecution {}
impl ExplicitScope for Transient {}

/// Implemented per injection-point type and scope: a type that needs an execution is not
/// `AllowedIn` [`Singleton`]. `#[injectable]` asserts it once per field or parameter, so the error
/// points at that field or parameter. `{Self}` is the injection point's type; the type that
/// cannot read it is the one declared `{S}`.
#[diagnostic::on_unimplemented(
    message = "a `{S}` type cannot read `{Self}`",
    label = "this injection point needs an execution",
    note = "declare the type #[injectable(execution)] or #[injectable(transient)]"
)]
pub trait AllowedIn<S: Scope> {}
