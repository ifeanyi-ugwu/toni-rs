//! One grammar for `#[module]`'s lists and the `DynamicModule` builder.
//!
//! `exports:` lists key types, as `.export::<K>()` does: a type bare or qualified, a marker, a
//! generic type with or without a turbofish, `dyn Trait`. `controllers:` takes a qualified path.
//! Each export is resolved by a consumer in a module that imports it, and an export is never checked
//! against its module's providers, so a spelling naming a key other than the binding's is found only
//! by that consumer failing startup.

use std::sync::Arc;

use ulo::di::{DynamicModule, Execution, Extension};
use ulo::http::Body;
use ulo::{UloFactory, controller, get, injectable, key, module, provide, routes};

use crate::common::TestServer;

mod keys {
    use super::*;

    key!(pub LibPort: u16);
}

key!(pub LibName: String);
key!(pub DynName: String);

pub mod helpers {
    #[ulo::injectable]
    pub struct Service {
        #[default("service".to_string())]
        pub label: String,
    }
}

#[derive(Clone)]
pub struct Marker;

#[derive(Clone)]
pub struct Handle<T>(pub T);

#[derive(Clone)]
pub struct Tag<T>(pub T);

#[derive(Clone)]
pub struct Current(pub &'static str);

pub trait Greeter: Send + Sync {
    fn greet(&self) -> &'static str;
}

pub struct English;

impl Greeter for English {
    fn greet(&self) -> &'static str {
        "hello"
    }
}

#[module(
    providers: [
        helpers::Service,
        provide!(async || Handle(Marker)),
        provide!(async || Tag(Marker)),
        provide!(LibName => "lib".to_string()),
        provide!(keys::LibPort => 7u16),
        provide!(dyn Greeter => value English),
        Extension::<Current>,
    ],
    exports: [
        helpers::Service,
        Handle<Marker>,
        Tag::<Marker>,
        LibName,
        keys::LibPort,
        dyn Greeter,
        Extension<Current>,
    ],
)]
pub struct LibraryModule;

fn dynamic_library() -> DynamicModule {
    DynamicModule::builder("DynLibrary")
        .provider(provide!(DynName => "dyn".to_string()))
        .export::<DynName>()
        .build()
}

#[injectable]
pub struct Consumer {
    #[inject]
    service: Arc<helpers::Service>,
    #[inject]
    _handle: Arc<Handle<Marker>>,
    #[inject]
    _tag: Arc<Tag<Marker>>,
    #[inject(LibName)]
    name: Arc<String>,
    #[inject(keys::LibPort)]
    port: Arc<u16>,
    #[inject(DynName)]
    dyn_name: Arc<String>,
    #[inject]
    greeter: Arc<dyn Greeter>,
}

#[injectable(scope = "execution")]
pub struct Reader {
    #[inject]
    current: Extension<Current>,
}

pub mod ctl {
    use super::*;

    #[controller("/grammar")]
    pub struct Hello {
        #[inject]
        consumer: Arc<Consumer>,
    }

    #[routes]
    impl Hello {
        #[get("/")]
        fn hello(&self) -> Body {
            let c = &self.consumer;
            Body::text(format!(
                "{} {} {} {} {}",
                c.service.label,
                c.name,
                c.port,
                c.dyn_name,
                c.greeter.greet()
            ))
        }
    }
}

#[module(
    imports: [LibraryModule, dynamic_library()],
    providers: [Consumer],
    controllers: [ctl::Hello],
)]
pub struct AppModule;

/// Imports the library for `Extension<Current>` alone, so this export is resolved apart from the
/// others.
#[module(imports: [LibraryModule], providers: [Reader])]
pub struct ReaderModule;

#[tokio::test]
async fn every_export_spelling_resolves_from_an_importing_module() {
    let server = TestServer::start(AppModule).await;

    let body = server
        .client()
        .get(server.url("/grammar"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    assert_eq!(body, "service lib 7 dyn hello");
}

#[tokio::test]
async fn an_extension_view_is_exported_like_any_provider() {
    let app = UloFactory::create(ReaderModule).await.unwrap();

    let reader = app
        .resolve::<Reader>(&Execution::standalone())
        .await
        .expect("the imported `Extension<Current>` resolves in the importing module");
    assert!(
        reader.current.get().is_none(),
        "nothing attached a `Current`"
    );
}
