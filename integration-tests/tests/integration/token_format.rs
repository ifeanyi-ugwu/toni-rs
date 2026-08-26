//! One token format across every registration and lookup path.
//!
//! A provider registered under its canonical `type_name` must be reachable from
//! every path that derives a token from a type: a bare `#[inject]` field, an
//! explicit `#[inject(Type)]`, a `provider_factory!` closure dependency, and
//! `resolve::<T>()` on the app. Generic written types are where the paths can
//! disagree — each test pins one pair.

use toni::toni_factory::ToniFactory;
use toni::{injectable, module, provider_factory, provider_value, ProviderContext};
use toni_config::{Config, ConfigModule, ConfigService};

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
        pub handle: Handle<Marker>,
    }

    #[module(
        providers: [
            provider_value!(Handle<Marker>, Handle(Marker)),
            Consumer,
        ],
    )]
    impl TestModule {}

    #[tokio::test]
    async fn a_generic_field_finds_a_type_registered_provider() {
        let app = ToniFactory::create(TestModule)
            .await
            .expect("a `Handle<Marker>` field must find the `Handle<Marker>` registration");

        app.resolve::<Consumer>(&ProviderContext::standalone())
            .await
            .expect("the consumer built, so it resolves");
    }
}

#[derive(Config, Clone)]
pub struct TokenTestConfig {
    #[env("TONI_TOKEN_TEST_NAME")]
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
        let app = ToniFactory::create(TestModule).await.unwrap();

        let service = app
            .resolve::<ConfigService<TokenTestConfig>>(&ProviderContext::standalone())
            .await
            .expect("`resolve` speaks the same token the registration used");

        assert_eq!(service.get_ref().name, "token-test");
    }
}

/// A factory closure's dependency lookup derives its token from the written
/// parameter type.
mod factory_dep_generic {
    use super::*;

    #[module(
        imports: [ConfigModule::<TokenTestConfig>::from_env().unwrap()],
        providers: [
            provider_factory!("CONFIGURED_NAME", |cfg: ConfigService<TokenTestConfig>| {
                cfg.get_ref().name.clone()
            }),
        ],
    )]
    impl TestModule {}

    #[tokio::test]
    async fn a_factory_dep_written_generic_is_found() {
        let app = ToniFactory::create(TestModule)
            .await
            .expect("the closure's `ConfigService<TokenTestConfig>` dep must be found");

        let name: String = app
            .get_by_token("CONFIGURED_NAME")
            .await
            .expect("the factory built from its dep");
        assert_eq!(name, "token-test");
    }
}

/// `#[inject(Type)]` and bare `#[inject]` on the same written type must agree.
mod explicit_inject_generic {
    use super::*;

    #[injectable]
    pub struct Consumer {
        #[inject(ConfigService<TokenTestConfig>)]
        pub explicit: ConfigService<TokenTestConfig>,
        #[inject]
        pub bare: ConfigService<TokenTestConfig>,
    }

    #[module(
        imports: [ConfigModule::<TokenTestConfig>::from_env().unwrap()],
        providers: [Consumer],
    )]
    impl TestModule {}

    #[tokio::test]
    async fn explicit_and_bare_inject_agree_on_the_token() {
        let app = ToniFactory::create(TestModule)
            .await
            .expect("both spellings must find the one registration");

        let consumer = app
            .resolve::<Consumer>(&ProviderContext::standalone())
            .await
            .unwrap();
        assert_eq!(
            consumer.explicit.get_ref().name,
            consumer.bare.get_ref().name
        );
    }
}

/// A field written with a qualified path must produce the same token as the
/// registration made from the bare ident.
mod qualified_path_inject {
    use super::*;

    pub mod helpers {
        #[toni::injectable]
        pub struct Service {
            #[default("qualified".to_string())]
            pub label: String,
        }
    }

    #[injectable]
    pub struct Consumer {
        #[inject]
        pub service: helpers::Service,
    }

    #[module(
        providers: [helpers::Service, Consumer],
    )]
    impl TestModule {}

    #[tokio::test]
    async fn a_qualified_written_type_finds_the_registration() {
        let app = ToniFactory::create(TestModule)
            .await
            .expect("`helpers::Service` and `Service` are the same type, so the same token");

        let consumer = app
            .resolve::<Consumer>(&ProviderContext::standalone())
            .await
            .unwrap();
        assert_eq!(consumer.service.label, "qualified");
    }
}

/// A `Token` const is a key like any other: the by-token lookup APIs accept it
/// directly.
mod const_token_lookup {
    use super::*;
    use toni::di::Token;

    const NAMED_VALUE: Token<String> = Token::new("NAMED_VALUE");

    #[module(
        providers: [provider_value!("NAMED_VALUE", "held".to_string())],
    )]
    impl TestModule {}

    #[tokio::test]
    async fn a_token_const_reaches_a_string_registration() {
        let app = ToniFactory::create(TestModule).await.unwrap();

        let value: String = app
            .get_by_token(NAMED_VALUE)
            .await
            .expect("the const names the same key the registration used");
        assert_eq!(value, "held");
    }
}
