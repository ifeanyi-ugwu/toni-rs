use futures::stream::{self, Stream};
use toni::{module, toni_factory::ToniFactory};
use toni_async_graphql::prelude::*;
use toni_axum::AxumAdapter;

// ---- Schema types --------------------------------------------------------

struct Query;

#[Object]
impl Query {
    async fn ping(&self) -> &str {
        "pong"
    }
}

struct Sub;

#[Subscription]
impl Sub {
    /// Counts down from `from` to 0, emitting one integer per item.
    async fn countdown(&self, from: i32) -> impl Stream<Item = i32> {
        stream::iter((0..=from).rev())
    }

    /// Emits the first `limit` non-negative integers.
    async fn integers(&self, limit: i32) -> impl Stream<Item = i32> {
        stream::iter(0..limit)
    }
}

// ---- Module setup --------------------------------------------------------

fn build_graphql_module() -> GraphQLModule<Query, EmptyMutation, Sub, DefaultContextBuilder> {
    let schema = Schema::build(Query, EmptyMutation, Sub).finish();
    GraphQLModule::for_root(schema, DefaultContextBuilder)
        .with_path("/graphql")
        .with_playground(true)
        .with_subscription_path("/graphql/ws")
}

#[module(
    imports: [build_graphql_module()],
    controllers: [],
    providers: [],
    exports: []
)]
impl AppModule {}

// ---- Main ----------------------------------------------------------------

#[tokio::main]
async fn main() {
    println!("GraphQL endpoint:      http://localhost:3000/graphql");
    println!("GraphQL Playground:    http://localhost:3000/graphql (open in browser)");
    println!("Subscription endpoint: ws://localhost:3000/graphql/ws");
    println!();
    println!("Test with wscat:");
    println!("  wscat -c ws://localhost:3000/graphql/ws");
    println!(r#"  > {{"type":"connection_init"}}"#);
    println!(
        r#"  > {{"type":"subscribe","id":"1","payload":{{"query":"subscription {{ countdown(from: 5) }}"}}}}"#
    );

    let mut app = ToniFactory::create(AppModule::module_definition()).await;
    app.use_http_adapter(AxumAdapter::new(), 3000, "127.0.0.1")
        .unwrap();
    app.start().await.unwrap();
}
