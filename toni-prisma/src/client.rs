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
        std::any::type_name::<C>().to_string()
    }

    async fn build(
        &self,
        _deps: FxHashMap<String, toni::traits_helpers::Injectable>,
    ) -> toni::traits_helpers::Injectable {
        let client = (self.connect)().await;
        toni::traits_helpers::Injectable::new(
            Arc::new(Box::new(PrismaClientProvider { client })),
            vec![],
        )
    }
}

struct PrismaClientProvider<C> {
    client: C,
}

#[async_trait]
impl<C: Send + Sync + Clone + 'static> Provider for PrismaClientProvider<C> {
    fn get_token(&self) -> String {
        std::any::type_name::<C>().to_string()
    }

    fn get_token_factory(&self) -> String {
        std::any::type_name::<C>().to_string()
    }

    async fn execute(
        &self,
        _params: Vec<Box<dyn Any + Send>>,
        _ctx: ProviderContext<'_>,
    ) -> Box<dyn Any + Send> {
        Box::new(self.client.clone())
    }
}
