use std::sync::Arc;

use crate::app::shared::AppShared;
use crate::error::{Closed, LookupError};
use crate::execution::{ExecOptions, Execution, ExecutionRef};
use crate::graph::ModuleId;
use crate::module::ModuleName;
use crate::module::meta::Meta;
use crate::site::Dep;

/// A handle to one module, for runtime lookups limited to what that module sees (§8.5).
///
/// Reached through `app.module::<M>()?`, `app.module_keyed::<M, Q>()?`, `handle.load(..)`, or as
/// a site naming the enclosing module. As a site read inside an execution it carries that
/// execution, so `get` reaches execution-scoped bindings too.
///
/// During `connect`, `get` for a singleton the eager walk has not built yet is refused with
/// `LookupError::NotReady`; once `connect` returns it cannot occur.
#[derive(Clone)]
pub struct ModuleRef {
    pub(crate) app: Arc<AppShared>,
    pub(crate) module: ModuleId,
    pub(crate) exec: Option<ExecutionRef>,
}

impl ModuleRef {
    pub(crate) fn new(app: Arc<AppShared>, module: ModuleId, exec: Option<ExecutionRef>) -> Self {
        ModuleRef { app, module, exec }
    }

    /// The binding `T` as this module sees it.
    pub async fn get<T: ?Sized + Send + Sync + 'static>(&self) -> Result<Dep<T>, LookupError> {
        todo!()
    }

    /// A standalone execution resolving with this module's visibility. Refused from Draining on
    /// with `Closed`; completion fires no cancellation (§6.3).
    pub async fn execute<F, R>(&self, opts: ExecOptions, f: F) -> Result<R, Closed>
    where
        F: AsyncFnOnce(&Execution) -> R,
    {
        todo!()
    }

    pub fn name(&self) -> ModuleName {
        todo!()
    }

    /// The metadata of type `T` this module wrote, if any.
    pub fn meta<T: Meta>(&self) -> Option<Arc<T>> {
        todo!()
    }
}
