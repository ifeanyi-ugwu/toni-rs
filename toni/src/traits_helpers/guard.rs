use async_trait::async_trait;

use crate::injector::Context;

#[async_trait]
pub trait Guard: Send + Sync {
    async fn can_activate(&self, context: &Context) -> bool;
}
