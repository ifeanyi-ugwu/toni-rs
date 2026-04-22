use async_graphql::Data;
use async_trait::async_trait;
use serde_json::Value;
use toni::WsClient;

/// Builds GraphQL context data for a subscription request.
///
/// Implement this to inject per-connection data (e.g. authenticated user, request headers)
/// into subscription resolvers via `ctx.data::<T>()`.
///
/// `client` carries the HTTP upgrade headers/query-params.
/// `init_payload` is the JSON payload from `connection_init`, if the client sent one —
/// useful for WS-level authentication tokens.
#[async_trait]
pub trait SubscriptionContextBuilder: Send + Sync + 'static {
    async fn build(&self, client: &WsClient, init_payload: Option<Value>) -> Data;
}

/// Default implementation — passes an empty data container to resolvers.
#[derive(Clone)]
pub struct DefaultSubscriptionContextBuilder;

#[async_trait]
impl SubscriptionContextBuilder for DefaultSubscriptionContextBuilder {
    async fn build(&self, _client: &WsClient, _init_payload: Option<Value>) -> Data {
        Data::default()
    }
}
