//! A GraphQL schema mounted at `/graphql`, with the playground served at the
//! same path.
//!
//! The schema is built and handed to `GraphQLModule::for_root` by value — it is
//! not resolved from DI, because a schema is a value the application owns
//! rather than a dependency it asks for.
//!
//!     cargo run -p ulo-graphql-async-graphql --example hello_world

use ulo::{UloFactory, module};
use ulo_graphql_async_graphql::{DefaultContextBuilder, prelude::*};
use ulo_http_hyper::AxumAdapter;

// Define Query type
struct Query;

#[Object]
impl Query {
    /// Simple hello world query
    async fn hello(&self) -> &str {
        "Hello, world!"
    }

    /// Query with arguments
    async fn greet(&self, name: String) -> String {
        format!("Hello, {}!", name)
    }

    /// Query that returns a custom type
    async fn user(&self, id: i32) -> User {
        User {
            id,
            name: format!("User {}", id),
            email: format!("user{}@example.com", id),
        }
    }
}

// Define a custom GraphQL type
#[derive(SimpleObject, Clone)]
struct User {
    id: i32,
    name: String,
    email: String,
}

fn build_graphql_module()
-> GraphQLModule<Query, EmptyMutation, EmptySubscription, DefaultContextBuilder> {
    let schema = Schema::build(Query, EmptyMutation, EmptySubscription).finish();
    GraphQLModule::for_root(schema, DefaultContextBuilder)
        .with_path("/graphql")
        .with_playground(true)
}

// Define the app module
#[module(
    imports: [build_graphql_module()],
    controllers: [],
    providers: [],
    exports: []
)]
impl AppModule {}

#[tokio::main]
async fn main() {
    println!("Starting GraphQL Hello World example...");
    println!("GraphQL endpoint: http://localhost:3000/graphql");
    println!("GraphQL Playground: http://localhost:3000/graphql (open in browser)");

    // Create Ulo app
    let mut app = UloFactory::create(AppModule).await.unwrap();

    app.use_http_adapter(AxumAdapter::new(), ("127.0.0.1", 3000))
        .unwrap();

    app.start().await.unwrap();
}
