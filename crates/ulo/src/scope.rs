//! Scope markers and the bounds the compiler checks against them (§3.3).
//!
//! What [`Auto`] means depends on the binding's role. For a provider it behaves exactly like
//! [`Singleton`], and depending on execution data is refused at `wire()`. For a controller or an
//! enhancer it resolves to singleton when nothing below it needs an execution, and to
//! per-execution otherwise. Writing `#[injectable(singleton)]` on a controller opts out of the
//! inference, so a controller declared that way that needs execution data is refused.

/// Built once during `connect` and shared across the application.
pub struct Singleton;

/// Built once per execution and shared inside it.
pub struct PerExecution;

/// Built fresh at every site.
pub struct Transient;

/// What `#[injectable]` means when no scope is written.
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
/// transient type fails to compile; a hook on an `Auto` type that the wiring pass infers
/// per-execution is refused at `wire()` (§6.2).
#[diagnostic::on_unimplemented(
    message = "lifecycle hooks are only allowed on singletons",
    label = "`{Self}` scope cannot have hooks",
    note = "remove the scope argument from #[injectable], or move the hook to a singleton"
)]
pub trait HookCapable: Scope {}

impl HookCapable for Singleton {}
impl HookCapable for Auto {}

/// Implemented per site type and scope: a site that needs an execution is not `AllowedIn`
/// [`Singleton`]. `#[injectable]` asserts it once per field or parameter, so the error points at
/// that site. `{Self}` is the site type; the type that cannot read it is the one declared `{S}`.
#[diagnostic::on_unimplemented(
    message = "a `{S}` type cannot read `{Self}`",
    label = "this site needs an execution",
    note = "declare the type #[injectable(execution)] or #[injectable(transient)]"
)]
pub trait AllowedIn<S: Scope> {}
