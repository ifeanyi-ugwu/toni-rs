use std::{any::Any, sync::Arc};

use async_trait::async_trait;

use crate::di::{Execution, ProviderScope};
use crate::spi::Provider;

/// Holds all contributions for a given multi-provider base token.
///
/// Each item is stored as `Arc<Arc<T>>` for the trait object `T` the collection's key names,
/// erased to `Arc<dyn Any + Send + Sync>`; the injection site downcasts back to `Arc<Arc<T>>` for
/// the trait object its field names and clones the inner `Arc`.
pub(super) struct MultiCollectionProvider {
    pub token: String,
    pub items: Vec<Arc<dyn Any + Send + Sync>>,
}

#[async_trait]
impl Provider for MultiCollectionProvider {
    fn token(&self) -> String {
        self.token.clone()
    }

    fn scope(&self) -> ProviderScope {
        ProviderScope::Singleton
    }

    async fn resolve(&self, _ctx: Execution) -> Box<dyn Any + Send> {
        Box::new(self.items.clone())
    }
}
