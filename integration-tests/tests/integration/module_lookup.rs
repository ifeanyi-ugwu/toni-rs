//! Modules are reachable from the app by type, or by name where the identity
//! is not a type. The handle is the same `ModuleRef` an injected field gets:
//! it resolves providers in that module's scope.

use toni::toni_factory::ToniFactory;
use toni::{injectable, module, provider_value, DynamicModule};
use toni_async_graphql::async_graphql::{EmptyMutation, EmptySubscription, Object, Schema};
use toni_async_graphql::{DefaultContextBuilder, GraphQLModule};
use toni_config::{Config, ConfigModule, ConfigService};

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
    #[env("TONI_MODULE_LOOKUP_NAME")]
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

fn dynamic() -> DynamicModule {
    DynamicModule::builder("LookupDyn")
        .provider(provider_value!("LOOKUP_VALUE", 7u32))
        .export_token("LOOKUP_VALUE")
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
/// providers and, in strict mode, nothing from other modules.
#[tokio::test]
async fn a_static_module_is_found_by_type() {
    let app = ToniFactory::create(AppModule).await.unwrap();

    let feature = app.get_module::<FeatureModule>().await.unwrap();
    let service: FeatureService = feature.get().await.unwrap();
    assert_eq!(service.label, "feature");

    assert!(
        feature.get::<RootService>().await.is_err(),
        "strict mode stays inside the module's scope"
    );
}

/// A generic library module is found by its written type.
#[tokio::test]
async fn a_generic_module_is_found_by_type() {
    let app = ToniFactory::create(AppModule).await.unwrap();

    let config = app
        .get_module::<ConfigModule<LookupConfig>>()
        .await
        .unwrap();
    let service: ConfigService<LookupConfig> = config.get().await.unwrap();
    assert_eq!(service.get_ref().name, "lookup");
}

/// A fingerprinted identity still matches its type when only one module
/// carries it.
#[tokio::test]
async fn a_fingerprinted_module_is_found_by_type() {
    let app = ToniFactory::create(AppModule).await.unwrap();

    app.get_module::<GraphQLModule<Query, EmptyMutation, EmptySubscription, DefaultContextBuilder>>()
        .await
        .expect("one fingerprinted module of this type matches");
}

#[module(imports: [gql("/gql-one"), gql("/gql-two")])]
impl TwoGqlModule {}

/// Two fingerprinted modules of one type are ambiguous, and the error says so.
#[tokio::test]
async fn two_fingerprinted_modules_of_one_type_are_ambiguous() {
    let app = ToniFactory::create(TwoGqlModule).await.unwrap();

    let err = app
        .get_module::<GraphQLModule<Query, EmptyMutation, EmptySubscription, DefaultContextBuilder>>()
        .await
        .expect_err("two modules share the type");
    assert!(
        err.to_string().contains("ambiguous"),
        "the error must say the type is ambiguous, got: {err}"
    );
}

/// A `DynamicModule`'s identity base is its builder-given name; the base
/// alone reaches it.
#[tokio::test]
async fn a_dynamic_module_is_found_by_its_base() {
    let app = ToniFactory::create(AppModule).await.unwrap();

    let dyn_module = app.get_module_by_id("LookupDyn").await.unwrap();
    let value: u32 = dyn_module.get_by_token("LOOKUP_VALUE").await.unwrap();
    assert_eq!(value, 7);
}

/// The full key an ambiguity error prints is an address: identity derives
/// from config, so a module value built with the same config renders the key
/// that reaches the imported one.
#[tokio::test]
async fn a_full_key_reaches_one_of_two_same_type_modules() {
    use toni::traits_helpers::ModuleMetadata;

    let app = ToniFactory::create(TwoGqlModule).await.unwrap();

    let key = gql("/gql-one").identity().key();
    app.get_module_by_id(&key)
        .await
        .expect("the rendered key addresses the module it fingerprints");
}

pub struct NeverImported;

/// A type nothing imported is a descriptive error, not a panic.
#[tokio::test]
async fn an_unimported_type_is_a_named_error() {
    let app = ToniFactory::create(AppModule).await.unwrap();

    let err = app
        .get_module::<NeverImported>()
        .await
        .expect_err("nothing imported this type");
    assert!(
        err.to_string().contains("NeverImported"),
        "the error must name the missing identity, got: {err}"
    );

    app.get_module_by_id("NoSuchBase")
        .await
        .expect_err("nothing carries this base");
}
