use std::{any::Any, future::Future, marker::PhantomData, sync::Arc};

use async_trait::async_trait;
use toni::{
    FxHashMap,
    traits_helpers::{Provider, ProviderContext, ProviderFactory},
};

pub(crate) struct PrismaClientFactory<C, F, Fut>
where
    C: Send + Sync + 'static,
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = C> + Send + 'static,
{
    pub connect: F,
    // Injection token for this client: the `C` type name for the default (`for_root`), or the
    // caller's chosen name for a `for_root_named` client.
    pub token: String,
    pub _client: PhantomData<C>,
}

#[async_trait]
impl<C, F, Fut> ProviderFactory for PrismaClientFactory<C, F, Fut>
where
    C: Send + Sync + Clone + 'static,
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = C> + Send + 'static,
{
    fn get_token(&self) -> String {
        self.token.clone()
    }

    async fn build(
        &self,
        _deps: FxHashMap<String, toni::traits_helpers::Injectable>,
    ) -> toni::traits_helpers::Injectable {
        let client = (self.connect)().await;
        toni::traits_helpers::Injectable::new(
            Arc::new(Box::new(PrismaClientProvider {
                client,
                token: self.token.clone(),
            })),
            vec![],
        )
    }
}

struct PrismaClientProvider<C> {
    client: C,
    token: String,
}

#[async_trait]
impl<C: Send + Sync + Clone + 'static> Provider for PrismaClientProvider<C> {
    fn get_token(&self) -> String {
        self.token.clone()
    }

    async fn execute(
        &self,
        _params: Vec<Box<dyn Any + Send>>,
        _ctx: ProviderContext,
    ) -> Box<dyn Any + Send> {
        Box::new(self.client.clone())
    }
}
