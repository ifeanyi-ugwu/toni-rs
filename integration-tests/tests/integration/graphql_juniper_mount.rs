//! The juniper integration mounts a schema, exports its service, and keys
//! module identity the same way the async-graphql one does.
//!
//! Both integrations answer the same `GraphQLModule` contract, and until this
//! file only one of them was proved: juniper was a dependency of nothing here,
//! so its two examples were all that compiled it against a running
//! application.
//!
//! What differs is deliberate and covered by its absence — juniper has no
//! subscription support, so there is no counterpart to
//! `graphql_subscriptions.rs`.

use juniper::{EmptyMutation, EmptySubscription, RootNode, graphql_object};
use ulo::UloFactory;
use ulo::di::Execution;
use ulo::{injectable, module};
use ulo_graphql_juniper::{DefaultContext, DefaultContextBuilder, GraphQLModule, GraphQLService};

pub struct Query;

#[graphql_object(context = DefaultContext)]
impl Query {
    fn ping() -> &'static str {
        "pong"
    }
}

type Schema =
    RootNode<'static, Query, EmptyMutation<DefaultContext>, EmptySubscription<DefaultContext>>;

type Svc = GraphQLService<
    Query,
    EmptyMutation<DefaultContext>,
    EmptySubscription<DefaultContext>,
    DefaultContextBuilder,
>;

fn schema() -> Schema {
    Schema::new(Query, EmptyMutation::new(), EmptySubscription::new())
}

fn graphql_at(
    path: &str,
) -> GraphQLModule<
    Query,
    EmptyMutation<DefaultContext>,
    EmptySubscription<DefaultContext>,
    DefaultContextBuilder,
> {
    GraphQLModule::for_root(schema(), DefaultContextBuilder).with_path(path)
}

#[injectable]
pub struct Consumer {
    #[inject]
    pub svc: Svc,
}

#[module(imports: [graphql_at("/graphql")], providers: [Consumer])]
impl InjectionModule {}

#[tokio::test]
async fn an_importing_module_injects_the_exported_service() {
    let app = UloFactory::create(InjectionModule)
        .await
        .expect("the exported service resolves across the module boundary");

    app.resolve::<Consumer>(&Execution::standalone())
        .await
        .expect("the consumer built, so it resolves");
}

#[module(imports: [graphql_at("/graphql"), graphql_at("/internal")])]
impl TwoPathsModule {}

/// Module identity is the type plus a fingerprint of the config, so the same
/// schema at two paths is two modules rather than one deduplicated import.
#[tokio::test]
async fn two_paths_mount_two_modules() {
    UloFactory::create(TwoPathsModule)
        .await
        .expect("one schema at two paths mounts both");
}

#[module(imports: [graphql_at("/graphql"), graphql_at("/graphql")])]
impl IdenticalImportModule {}

/// An identical import is the diamond case: same type, same config, one
/// module. Registering the path twice would be a bind error.
#[tokio::test]
async fn an_identical_import_dedups() {
    UloFactory::create(IdenticalImportModule)
        .await
        .expect("an identical import dedups instead of colliding on the path");
}
