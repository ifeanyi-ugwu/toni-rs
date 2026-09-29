use std::any::Any;
use std::sync::Arc;

use super::BroadcastService;
use crate::FxHashMap;
use crate::async_trait;
use crate::di::Execution;
use crate::di::ProviderScope;
use crate::error::{BuildResult, ResolutionError};
use crate::spi::{Provider, ProviderFactory};
/// Singleton provider that hands out clones of the pre-built `BroadcastService`.
pub(crate) struct BroadcastServiceProvider {
    instance: BroadcastService,
}

#[async_trait]
impl Provider for BroadcastServiceProvider {
    fn token(&self) -> String {
        crate::di::token_of::<BroadcastService>()
    }

    async fn resolve(&self, _ctx: Execution) -> Result<Box<dyn Any + Send>, ResolutionError> {
        Ok(Box::new(self.instance.clone()))
    }

    fn scope(&self) -> ProviderScope {
        ProviderScope::Singleton
    }
}

impl Clone for BroadcastServiceProvider {
    fn clone(&self) -> Self {
        Self {
            instance: self.instance.clone(),
        }
    }
}

pub(crate) struct BroadcastServiceManager;

#[async_trait]
impl ProviderFactory for BroadcastServiceManager {
    fn token(&self) -> String {
        crate::di::token_of::<BroadcastService>()
    }

    async fn build(
        &self,
        _deps: FxHashMap<String, crate::spi::Registration>,
    ) -> BuildResult<crate::spi::Registration> {
        Ok(crate::spi::Registration::new(
            Arc::new(BroadcastServiceProvider {
                instance: BroadcastService::new(),
            }) as Arc<dyn Provider>,
            vec![],
        ))
    }
}
