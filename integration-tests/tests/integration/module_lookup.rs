//! Modules are reachable from the app by type, or by name where the identity
//! is not a type. The handle is the same `ModuleRef` an injected field gets:
//! it resolves providers in that module's scope.

use ulo::UloFactory;
use ulo::di::DynamicModule;
use ulo::di::ResolutionError;
use ulo::{injectable, key, module, provide};
use ulo_config::{Config, ConfigModule, ConfigService};
use ulo_graphql_async_graphql::async_graphql::{EmptyMutation, EmptySubscription, Object, Schema};
use ulo_graphql_async_graphql::{DefaultContextBuilder, GraphQLModule};

#[injectable]
pub struct FeatureService {
    #[default("feature".to_string())]
    pub label: String,
}

#[module(providers: [FeatureService], exports: [FeatureService])]
impl FeatureModule {}

#[injectable]
pub struct RootService {
    #[default("root".to_string())]
    pub label: String,
}

#[derive(Config, Clone)]
pub struct LookupConfig {
    #[env("ULO_MODULE_LOOKUP_NAME")]
    #[default("lookup".to_string())]
    pub name: String,
}

#[derive(Clone)]
struct Query;

#[Object]
impl Query {
    async fn ping(&self) -> &'static str {
        "pong"
    }
}

fn gql(
    path: &str,
) -> GraphQLModule<Query, EmptyMutation, EmptySubscription, DefaultContextBuilder> {
    let schema = Schema::build(Query, EmptyMutation, EmptySubscription).finish();
    GraphQLModule::for_root(schema, DefaultContextBuilder).with_path(path)
}

key!(LookupValue: u32);

fn dynamic() -> DynamicModule {
    DynamicModule::builder("LookupDyn")
        .provider(provide!(LookupValue => 7u32))
        .export::<LookupValue>()
        .build()
}

#[module(
    imports: [
        FeatureModule,
        ConfigModule::<LookupConfig>::from_env().unwrap(),
        gql("/lookup-gql"),
        dynamic(),
    ],
    providers: [RootService],
)]
impl AppModule {}

/// A `#[module]` type is its identity: the handle resolves that module's
/// providers and, by default, nothing from other modules.
#[tokio::test]
async fn a_static_module_is_found_by_type() {
    let app = UloFactory::create(AppModule).await.unwrap();

    let feature = app.get_module::<FeatureModule>().await.unwrap();
    let service = feature.get::<FeatureService>().await.unwrap();
    assert_eq!(service.label, "feature");

    match feature.get::<RootService>().await.err() {
        Some(ResolutionError::ProviderNotFound { module, .. }) => assert_eq!(
            module.as_deref().map(|m| m.contains("FeatureModule")),
            Some(true),
            "a lookup in the current module reports the one module it searched"
        ),
        other => panic!("a lookup stays inside the module's scope, got: {other:?}"),
    }
}

/// A generic library module is found by its written type.
#[tokio::test]
async fn a_generic_module_is_found_by_type() {
    let app = UloFactory::create(AppModule).await.unwrap();

    let config = app
        .get_module::<ConfigModule<LookupConfig>>()
        .await
        .unwrap();
    let service = config.get::<ConfigService<LookupConfig>>().await.unwrap();
    assert_eq!(service.get_ref().name, "lookup");
}

/// A fingerprinted identity still matches its type when only one module
/// carries it.
#[tokio::test]
async fn a_fingerprinted_module_is_found_by_type() {
    let app = UloFactory::create(AppModule).await.unwrap();

    app.get_module::<GraphQLModule<Query, EmptyMutation, EmptySubscription, DefaultContextBuilder>>()
        .await
        .expect("one fingerprinted module of this type matches");
}

#[module(imports: [gql("/gql-one"), gql("/gql-two")])]
impl TwoGqlModule {}

/// Two fingerprinted modules of one type are ambiguous, and the error carries
/// both keys.
#[tokio::test]
async fn two_fingerprinted_modules_of_one_type_are_ambiguous() {
    let app = UloFactory::create(TwoGqlModule).await.unwrap();

    let err = app
        .get_module::<GraphQLModule<Query, EmptyMutation, EmptySubscription, DefaultContextBuilder>>()
        .await
        .expect_err("two modules share the type");

    match err {
        ResolutionError::AmbiguousModule { candidates, .. } => {
            assert_eq!(
                candidates.len(),
                2,
                "both modules are named: {candidates:?}"
            )
        }
        other => panic!("two modules share the type, got: {other:?}"),
    }
}

/// The keys an ambiguity carries are addresses: a caller reads them off the
/// error and resolves each module without parsing a message.
#[tokio::test]
async fn an_ambiguity_hands_back_keys_that_resolve() {
    let app = UloFactory::create(TwoGqlModule).await.unwrap();

    let err = app
        .get_module::<GraphQLModule<Query, EmptyMutation, EmptySubscription, DefaultContextBuilder>>()
        .await
        .expect_err("two modules share the type");

    let ResolutionError::AmbiguousModule { candidates, .. } = err else {
        panic!("two modules share the type");
    };

    assert_eq!(
        candidates.len(),
        2,
        "the recovery needs both keys to be there: {candidates:?}"
    );
    for key in &candidates {
        app.get_module_by_id(key)
            .await
            .unwrap_or_else(|e| panic!("key `{key}` from the error must resolve, got: {e}"));
    }
}

/// A `DynamicModule`'s identity base is its builder-given name; the base
/// alone reaches it.
#[tokio::test]
async fn a_dynamic_module_is_found_by_its_base() {
    let app = UloFactory::create(AppModule).await.unwrap();

    let dyn_module = app.get_module_by_id("LookupDyn").await.unwrap();
    let value = dyn_module.get_key::<LookupValue>().await.unwrap();
    assert_eq!(*value, 7);
}

/// The full key an ambiguity error prints is an address: identity derives
/// from config, so a module value built with the same config renders the key
/// that reaches the imported one.
#[tokio::test]
async fn a_full_key_reaches_one_of_two_same_type_modules() {
    use ulo::di::ModuleMetadata;

    let app = UloFactory::create(TwoGqlModule).await.unwrap();

    let key = gql("/gql-one").identity().key();
    app.get_module_by_id(&key)
        .await
        .expect("the rendered key addresses the module it fingerprints");
}

pub struct NeverImported;

/// A type nothing imported is a descriptive error, not a panic.
#[tokio::test]
async fn an_unimported_type_is_a_named_error() {
    let app = UloFactory::create(AppModule).await.unwrap();

    let err = app
        .get_module::<NeverImported>()
        .await
        .expect_err("nothing imported this type");
    match err {
        ResolutionError::ModuleNotFound { id } => assert!(
            id.contains("NeverImported"),
            "the error must name the missing identity, got: {id}"
        ),
        other => panic!("an unimported type is a module-not-found, got: {other:?}"),
    }

    app.get_module_by_id("NoSuchBase")
        .await
        .expect_err("nothing carries this base");
}
