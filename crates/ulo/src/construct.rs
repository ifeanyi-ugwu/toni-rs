use std::error::Error;
use std::fmt;
use std::future::Future;

use crate::error::LookupError;
use crate::hooks::Hooks;
use crate::resolver::Resolver;
use crate::scope::Scope;
use crate::site::Sites;
use crate::timer::{BoxError, Bound};

/// A type the container builds, with its dependencies read from its sites.
///
/// `#[injectable]` writes this impl from a struct's fields or from a constructor's parameters;
/// a hand-written impl declares the same sites in [`sites`](Construct::sites) and reads them in
/// [`construct`](Construct::construct). Registration cannot override [`Construct::Scope`], which
/// is what keeps the compile-time hook check sound.
pub trait Construct: Sized + Send + Sync + 'static {
    type Scope: Scope;

    /// How long `construct` may take (§3.9). `#[injectable(timeout = ..)]` writes `After`.
    ///
    /// It bounds `construct` wherever the instance is built: during `connect` for a singleton,
    /// and inside the call for an execution-scoped or transient binding. Expiry reports as
    /// `FailureReason::TimedOut`.
    const CONSTRUCT_TIMEOUT: Bound = Bound::Default;

    /// Declared sites, used by the wiring pass. Generated from the fields or ctor params.
    fn sites(s: &mut Sites);

    /// Build the instance. Async and fallible.
    ///
    /// A failed site read propagates as the dependency's own error through `?`; the
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
/// A `Site` error passes through unchanged and names the deeper key; only `Failed` becomes
/// `Construct { reason: Errored(..) }`, redacted when the core stores it. Panics and timeouts
/// are not variants: the core observes them from outside the constructor and records them as
/// `FailureReason::{Panicked, TimedOut}`.
///
/// There is no blanket `From<E: Error>`: it would overlap `From<LookupError>`.
#[non_exhaustive]
pub enum ConstructError {
    /// A dependency read failed; propagated as the dependency's own error.
    Site(LookupError),
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
        ConstructError::Site(e)
    }
}

impl fmt::Debug for ConstructError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        todo!()
    }
}

impl fmt::Display for ConstructError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConstructError::Site(e) => fmt::Display::fmt(e, f),
            ConstructError::Failed(e) => fmt::Display::fmt(e, f),
        }
    }
}

impl Error for ConstructError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        todo!()
    }
}
