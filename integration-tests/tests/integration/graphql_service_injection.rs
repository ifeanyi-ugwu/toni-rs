//! `GraphQLModule` exports its service under the service's type; a module that
//! imports it injects the service by that type. Export-instance resolution keys
//! on the provider's own token, so the declared export and the built instance
//! meet.

use ulo::UloFactory;
use ulo::di::Execution;
use ulo::{injectable, module};
use ulo_graphql_async_graphql::async_graphql::{EmptyMutation, EmptySubscription, Object, Schema};
use ulo_graphql_async_graphql::{DefaultContextBuilder, GraphQLModule, GraphQLService};

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
    #[inject]
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
    let app = UloFactory::create(TestModule)
        .await
        .expect("the exported service resolves across the module boundary");

    app.resolve::<Consumer>(&Execution::standalone())
        .await
        .expect("the consumer built, so it resolves");
}
