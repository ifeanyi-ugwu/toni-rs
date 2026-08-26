//! `GraphQLModule` exports `"GraphQLService"`; a module that imports it can
//! inject the service. Export-instance resolution keys on the provider's own
//! token, so the declared export and the built instance meet.

use toni::toni_factory::ToniFactory;
use toni::{injectable, module, ProviderContext};
use toni_async_graphql::async_graphql::{EmptyMutation, EmptySubscription, Object, Schema};
use toni_async_graphql::{DefaultContextBuilder, GraphQLModule, GraphQLService};

#[derive(Clone)]
struct Query;

#[Object]
impl Query {
    async fn ping(&self) -> &'static str {
        "pong"
    }
}

type Svc = GraphQLService<Query, EmptyMutation, EmptySubscription, DefaultContextBuilder>;

#[injectable]
pub struct Consumer {
    #[inject("GraphQLService")]
    pub svc: Svc,
}

fn graphql() -> GraphQLModule<Query, EmptyMutation, EmptySubscription, DefaultContextBuilder> {
    let schema = Schema::build(Query, EmptyMutation, EmptySubscription).finish();
    GraphQLModule::for_root(schema, DefaultContextBuilder)
}

#[module(
    imports: [graphql()],
    providers: [Consumer],
)]
impl TestModule {}

#[tokio::test]
async fn an_importing_module_injects_the_exported_service() {
    let app = ToniFactory::create(TestModule)
        .await
        .expect("the exported service resolves across the module boundary");

    app.resolve::<Consumer>(&ProviderContext::standalone())
        .await
        .expect("the consumer built, so it resolves");
}
