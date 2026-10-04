//! A GraphQL subscription over WebSocket, at a path of its own.
//!
//! `with_subscription_path` mounts the `graphql-transport-ws` endpoint beside
//! the query path; the subprotocol is enforced, so a client connecting without
//! it is refused rather than left waiting.
//!
//!     cargo run -p ulo-graphql-async-graphql --example subscriptions
//!     # queries at http://localhost:3000/graphql
//!     # subscriptions at ws://localhost:3000/graphql/ws

use futures::stream::{self, Stream};
use ulo::{UloFactory, module};
use ulo_graphql_async_graphql::prelude::*;
use ulo_http_hyper::AxumAdapter;

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

    let mut app = UloFactory::create(AppModule).await.unwrap();
    app.use_http_adapter(AxumAdapter::new(), ("127.0.0.1", 3000))
        .unwrap();
    app.start().await.unwrap();
}
