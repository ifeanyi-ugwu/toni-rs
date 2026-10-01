use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};

use async_lock::OnceCell;

use crate::binding::Instance;
use crate::error::LookupError;
use crate::graph::BindingId;

/// One instance per execution-scoped binding per execution. Two sites resolving the same
/// binding concurrently get one instance: both await one cell. A failed build is not cached.
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
        todo!()
    }
}
