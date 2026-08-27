use std::any::Any;
use std::sync::Arc;

use crate::FxHashMap;
use crate::async_trait;
use crate::provider_scope::ProviderScope;
use crate::traits_helpers::{Provider, ProviderContext, ProviderFactory};

use super::BroadcastService;

/// Singleton provider that hands out clones of the pre-built `BroadcastService`.
pub(crate) struct BroadcastServiceProvider {
    instance: BroadcastService,
}

#[async_trait]
impl Provider for BroadcastServiceProvider {
    fn get_token(&self) -> String {
        crate::di::token_of::<BroadcastService>()
    }

    async fn execute(
        &self,
        _params: Vec<Box<dyn Any + Send>>,
        _ctx: ProviderContext,
    ) -> Box<dyn Any + Send> {
        Box::new(self.instance.clone())
    }

    fn get_scope(&self) -> ProviderScope {
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
    fn get_token(&self) -> String {
        crate::di::token_of::<BroadcastService>()
    }

    async fn build(
        &self,
        _deps: FxHashMap<String, crate::traits_helpers::Injectable>,
    ) -> crate::traits_helpers::Injectable {
        crate::traits_helpers::Injectable::new(
            Arc::new(Box::new(BroadcastServiceProvider {
                instance: BroadcastService::new(),
            }) as Box<dyn Provider>),
            vec![],
        )
    }
}
