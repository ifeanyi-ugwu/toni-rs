use std::{
    any::Any,
    future::Future,
    marker::PhantomData,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use async_trait::async_trait;
use ulo::{
    FxHashMap,
    di::{Execution, ResolutionError},
    spi::{BuildResult, Provider, ProviderFactory},
};

pub(crate) struct PrismaClientFactory<C, F, Fut>
where
    C: Send + Sync + 'static,
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = C> + Send + 'static,
{
    pub connect: F,
    // Injection token for this client: `C`'s own for the default (`for_root`), or the marker's
    // for a `for_root_keyed` client.
    pub token: String,
    // This call's place among the process's registrations. The client is configured by a closure,
    // which cannot be compared, so no two calls are the same registration: each one's module gets
    // an identity of its own, and two of one client type collide on the global export at startup
    // rather than one being dropped as a repeat of the other.
    pub registration: u64,
    pub _client: PhantomData<C>,
}

static REGISTRATIONS: AtomicU64 = AtomicU64::new(0);

pub(crate) fn next_registration() -> u64 {
    REGISTRATIONS.fetch_add(1, Ordering::Relaxed)
}

#[async_trait]
impl<C, F, Fut> ProviderFactory for PrismaClientFactory<C, F, Fut>
where
    C: Send + Sync + Clone + 'static,
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = C> + Send + 'static,
{
    fn token(&self) -> String {
        self.token.clone()
    }

    fn identity_hint(&self) -> Option<String> {
        Some(self.registration.to_string())
    }

    async fn build(
        &self,
        _deps: FxHashMap<String, ulo::spi::Registration>,
    ) -> BuildResult<ulo::spi::Registration> {
        let client = (self.connect)().await;
        Ok(ulo::spi::Registration::new(
            Arc::new(PrismaClientProvider {
                client,
                token: self.token.clone(),
            }),
            vec![],
        ))
    }
}

struct PrismaClientProvider<C> {
    client: C,
    token: String,
}

#[async_trait]
impl<C: Send + Sync + Clone + 'static> Provider for PrismaClientProvider<C> {
    fn token(&self) -> String {
        self.token.clone()
    }

    async fn resolve(&self, _ctx: Execution) -> Result<Box<dyn Any + Send>, ResolutionError> {
        Ok(Box::new(self.client.clone()))
    }
}
