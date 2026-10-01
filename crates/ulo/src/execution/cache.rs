use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};

use async_lock::OnceCell;

use crate::binding::Instance;
use crate::error::LookupError;
use crate::graph::BindingId;

/// One instance per execution-scoped binding per execution. Two sites resolving the same
/// binding concurrently get one instance: both await one cell. A failed build is not cached.
///
/// The cell holds the instance as the recipe built it; `AppShared::obtain` widens a
/// contribution to its collection's type on the way out, as it does for a stored singleton.
#[derive(Default)]
pub(crate) struct ExecCache {
    cells: Mutex<HashMap<BindingId, Arc<OnceCell<Instance>>>>,
}

impl ExecCache {
    pub(crate) async fn get_or_build<F, Fut>(&self, id: BindingId, build: F) -> Result<Instance, LookupError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Instance, LookupError>>,
    {
        // The map lock is released before the build is awaited: the build reads other sites,
        // which come back here for their own cells.
        let cell = {
            let mut cells = self.cells.lock().unwrap_or_else(PoisonError::into_inner);
            Arc::clone(cells.entry(id).or_default())
        };
        let instance = cell.get_or_try_init(build).await?;
        Ok(Arc::clone(instance))
    }
}
