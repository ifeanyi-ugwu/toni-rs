use async_graphql::Data;
use async_trait::async_trait;
use toni::WsClient;

/// Builds GraphQL context data from an established WebSocket client connection.
///
/// Implement this to inject per-connection data (e.g. authenticated user, request headers)
/// into subscription resolvers via `ctx.data::<T>()`. The `WsClient` carries the upgrade
/// request's headers and query string, which is where auth tokens typically live.
#[async_trait]
pub trait SubscriptionContextBuilder: Send + Sync + 'static {
    async fn build(&self, client: &WsClient) -> Data;
}

/// Default implementation — passes an empty data container to resolvers.
#[derive(Clone)]
pub struct DefaultSubscriptionContextBuilder;

#[async_trait]
impl SubscriptionContextBuilder for DefaultSubscriptionContextBuilder {
    async fn build(&self, _client: &WsClient) -> Data {
        Data::default()
    }
}
