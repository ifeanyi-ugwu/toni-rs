use crate::error::LookupError;
use crate::execution::ExecutionRef;
use crate::module::handle::ModuleRef;
use crate::resolver::Resolver;
use crate::scope::{AllowedIn, Auto, PerExecution, Scope, Transient};
use crate::site::{Site, SiteDesc};

/// The module the reading binding belongs to, carrying the current execution when there is one.
impl Site for ModuleRef {
    fn describe(d: &mut SiteDesc) {
        d.module();
    }

    async fn read(r: &Resolver<'_>) -> Result<Self, LookupError> {
        Ok(r.module())
    }
}

impl<S: Scope> AllowedIn<S> for ModuleRef {}

/// A handle to the current execution: its cancellation, deadline and extensions. Needs an
/// execution, so an explicit singleton cannot read one.
impl Site for ExecutionRef {
    fn describe(d: &mut SiteDesc) {
        d.execution();
    }

    async fn read(r: &Resolver<'_>) -> Result<Self, LookupError> {
        r.execution()
    }
}

impl AllowedIn<PerExecution> for ExecutionRef {}
impl AllowedIn<Transient> for ExecutionRef {}
impl AllowedIn<Auto> for ExecutionRef {}
