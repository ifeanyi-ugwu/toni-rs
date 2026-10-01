use std::error::Error;
use std::fmt;
use std::future::Future;

use crate::dependency::Dependencies;
use crate::error::LookupError;
use crate::hooks::Hooks;
use crate::resolver::Resolver;
use crate::scope::Scope;
use crate::timer::{BoxError, Bound};

/// A type the container builds, with its dependencies read from its injection points.
///
/// `#[injectable]` writes this impl from a struct's fields or from a constructor's parameters;
/// a hand-written impl declares the same injection points in
/// [`dependencies`](Construct::dependencies) and reads them in
/// [`construct`](Construct::construct). Registration cannot override [`Construct::Scope`],
/// which is what keeps the compile-time hook check sound.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a type the container can construct",
    label = "the container cannot build this",
    note = "add #[injectable] to the type, or bind it with a factory"
)]
pub trait Construct: Sized + Send + Sync + 'static {
    type Scope: Scope;

    /// How long `construct` may take (§3.9). `#[injectable(timeout = ..)]` writes `After`.
    ///
    /// It bounds `construct` wherever the instance is built: during `connect` for a singleton,
    /// and inside the call for an execution-scoped or transient binding. Expiry reports as
    /// `FailureReason::TimedOut`.
    const CONSTRUCT_TIMEOUT: Bound = Bound::Default;

    /// Declared injection points, used by the wiring pass. Generated from the fields or ctor
    /// params.
    fn dependencies(d: &mut Dependencies);

    /// Build the instance. Async and fallible.
    ///
    /// A failed read propagates as the dependency's own error through `?`; the
    /// constructor's own error goes through [`ConstructError::failed`]:
    /// `.map_err(ConstructError::failed)?`.
    fn construct(r: &Resolver<'_>) -> impl Future<Output = Result<Self, ConstructError>> + Send;

    /// Lifecycle hooks this type implements. The macro fills this in by probing;
    /// a hand-written impl calls the `Hooks<Self>` methods it implements (§9.1),
    /// or `ulo::hooks!(h)` to probe all five.
    fn hooks(_h: &mut Hooks<Self>) {}
}

/// Why a constructor did not return its instance.
///
/// A `Dependency` error passes through unchanged and names the deeper key; only `Failed` becomes
/// `Construct { reason: Errored(..) }`, redacted when the core stores it. Panics and timeouts
/// are not variants: the core observes them from outside the constructor and records them as
/// `FailureReason::{Panicked, TimedOut}`.
///
/// There is no blanket `From<E: Error>`: it would overlap `From<LookupError>`.
#[non_exhaustive]
pub enum ConstructError {
    /// A dependency read failed; propagated as the dependency's own error.
    Dependency(LookupError),
    /// The constructor's own error; redacted by the core when stored.
    Failed(BoxError),
}

impl ConstructError {
    pub fn failed(e: impl Into<BoxError>) -> Self {
        ConstructError::Failed(e.into())
    }
}

impl From<LookupError> for ConstructError {
    fn from(e: LookupError) -> Self {
        ConstructError::Dependency(e)
    }
}

impl fmt::Debug for ConstructError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConstructError::Dependency(e) => f.debug_tuple("Dependency").field(e).finish(),
            ConstructError::Failed(e) => f.debug_tuple("Failed").field(e).finish(),
        }
    }
}

impl fmt::Display for ConstructError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConstructError::Dependency(e) => fmt::Display::fmt(e, f),
            ConstructError::Failed(e) => fmt::Display::fmt(e, f),
        }
    }
}

/// `Display` writes the wrapped error's own text, so `source` continues from that error's source
/// rather than repeating it to a reporter walking the chain.
impl Error for ConstructError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            ConstructError::Dependency(e) => e.source(),
            ConstructError::Failed(e) => e.source(),
        }
    }
}
