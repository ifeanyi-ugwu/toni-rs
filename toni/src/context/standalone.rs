use std::sync::Arc;

use crate::context::Metadata;

use super::{CancellationToken, Extensions, HandlerContext, shared::SharedState};

/// An execution with no transport behind it.
///
/// A CLI command, a cron tick, a background job, a test — one unit of work that
/// arrived over nothing. It carries what any execution carries, so a
/// request-scoped provider can be built into it and shared by everything
/// resolved in it:
///
/// ```rust,ignore
/// let execution = ProviderContext::standalone();
/// let repo = app.resolve::<Repo>(&execution).await?;
/// let audit = app.resolve::<AuditLog>(&execution).await?;  // same request-scoped deps
/// ```
///
/// The execution lasts as long as the handle. Dropping it ends it, which is what
/// drops the instances built into it.
///
/// Nothing dispatches to a standalone execution: there is no handler and no
/// enhancer chain. It is the state an execution carries, offered to a caller who
/// is doing the work themselves.
#[derive(Clone)]
pub struct StandaloneContext {
    inner: Arc<SharedState>,
}

impl StandaloneContext {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(SharedState::new(None)),
        }
    }
}

impl Default for StandaloneContext {
    fn default() -> Self {
        Self::new()
    }
}

impl HandlerContext for StandaloneContext {
    /// Always `None`: metadata is what a handler declared about itself, and there
    /// is no handler here.
    fn metadata(&self) -> Option<&Metadata> {
        self.inner.metadata.as_deref()
    }

    fn extensions(&self) -> &Extensions {
        &self.inner.extensions
    }

    fn cache(&self) -> &crate::traits_helpers::ExecutionCache {
        &self.inner.cache
    }

    fn cancellation(&self) -> &CancellationToken {
        &self.inner.cancellation
    }
}
