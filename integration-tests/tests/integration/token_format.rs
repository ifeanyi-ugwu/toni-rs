//! One token format across every registration and lookup path.
//!
//! A provider registered under its canonical `type_name` must be reachable from
//! every path that derives a token from a type: a bare `#[inject]` field, a
//! factory closure's parameter, and `resolve::<T>()` on the app. Generic written
//! types are where the paths can disagree — each test pins one pair.

use std::sync::Arc;
use ulo::UloFactory;
use ulo::di::Execution;
use ulo::{injectable, key, module, provide};
use ulo_config::{Config, ConfigModule, ConfigService};

#[derive(Clone)]
pub struct Marker;

#[derive(Clone)]
pub struct Handle<T>(pub T);

/// The shape every DB integration uses: the library registers its handle under
/// `type_name::<Handle<Marker>>()`, the app writes the generic type in a field.
mod bare_inject_generic {
    use super::*;

    #[injectable]
    pub struct Consumer {
        #[inject]
        pub handle: Arc<Handle<Marker>>,
    }

    #[module(
        providers: [
            provide!(async || Handle(Marker)),
            Consumer,
        ],
    )]
    impl TestModule {}

    #[tokio::test]
    async fn a_generic_field_finds_a_type_registered_provider() {
        let app = UloFactory::create(TestModule)
            .await
            .expect("a `Handle<Marker>` field must find the `Handle<Marker>` registration");

        app.resolve::<Consumer>(&Execution::standalone())
            .await
            .expect("the consumer built, so it resolves");
    }
}

#[derive(Config, Clone)]
pub struct TokenTestConfig {
    #[env("ULO_TOKEN_TEST_NAME")]
    #[default("token-test".to_string())]
    pub name: String,
}

/// `resolve` derives its token from the type; it must find what the library
/// registered.
mod resolve_generic {
    use super::*;

    #[module(
        imports: [ConfigModule::<TokenTestConfig>::from_env().unwrap()],
    )]
    impl TestModule {}

    #[tokio::test]
    async fn resolve_finds_a_library_registered_generic() {
        let app = UloFactory::create(TestModule).await.unwrap();

        let service = app
            .resolve::<ConfigService<TokenTestConfig>>(&Execution::standalone())
            .await
            .expect("`resolve` speaks the same token the registration used");

        assert_eq!(service.get_ref().name, "token-test");
    }
}

/// A factory closure's dependency lookup derives its token from the written
/// parameter type.
mod factory_dep_generic {
    use super::*;

    key!(ConfiguredName: String);

    #[module(
        imports: [ConfigModule::<TokenTestConfig>::from_env().unwrap()],
        providers: [
            provide!(ConfiguredName => async |cfg: ConfigService<TokenTestConfig>| {
                cfg.get_ref().name.clone()
            }),
        ],
    )]
    impl TestModule {}

    #[tokio::test]
    async fn a_factory_dep_written_generic_is_found() {
        let app = UloFactory::create(TestModule)
            .await
            .expect("the closure's `ConfigService<TokenTestConfig>` dep must be found");

        let name = app
            .get_key::<ConfiguredName>()
            .await
            .expect("the factory built from its dep");
        assert_eq!(*name, "token-test");
    }
}

/// A field written with a qualified path must produce the same token as the
/// registration made from the bare ident.
mod qualified_path_inject {
    use super::*;

    pub mod helpers {
        #[ulo::injectable]
        pub struct Service {
            #[default("qualified".to_string())]
            pub label: String,
        }
    }

    #[injectable]
    pub struct Consumer {
        #[inject]
        pub service: Arc<helpers::Service>,
    }

    #[module(
        providers: [helpers::Service, Consumer],
    )]
    impl TestModule {}

    #[tokio::test]
    async fn a_qualified_written_type_finds_the_registration() {
        let app = UloFactory::create(TestModule)
            .await
            .expect("`helpers::Service` and `Service` are the same type, so the same token");

        let consumer = app
            .resolve::<Consumer>(&Execution::standalone())
            .await
            .unwrap();
        assert_eq!(consumer.service.label, "qualified");
    }
}

/// A marker is a key like any other: the `_key` lookups take it, and its `Value` fixes the result
/// type.
mod marker_lookup {
    use super::*;

    key!(NamedValue: String);

    #[module(
        providers: [provide!(NamedValue => "held".to_string())],
    )]
    impl TestModule {}

    #[tokio::test]
    async fn a_marker_reaches_its_registration() {
        let app = UloFactory::create(TestModule).await.unwrap();

        let value = app
            .get_key::<NamedValue>()
            .await
            .expect("the marker names the same key the registration used");
        assert_eq!(*value, "held");
    }
}
