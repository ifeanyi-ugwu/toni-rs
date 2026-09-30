//! `provide!(into key => source)` collects every contribution under one key
//! into the `Vec<Arc<dyn Trait>>` a consumer injects.
//!
//! A multi-provider's failure is quiet by construction: a contribution that
//! never registers yields a shorter vec, and a consumer iterating it cannot
//! tell. Each contribution form is asserted by what the collection contains,
//! and the empty and single-element cases are covered because they are where a
//! collection type degrades into something else.
use crate::common::TestServer;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use ulo::http::Body;
use ulo::{UloFactory, controller, get, injectable, key, module, new, provide, routes};
trait Plugin: Send + Sync {
    fn name(&self) -> &'static str;
}

#[tokio::test]
async fn multi_type_path_collects_all_contributions() {
    #[injectable]
    pub struct PluginA {}
    impl PluginA {}

    impl Plugin for PluginA {
        fn name(&self) -> &'static str {
            "alpha"
        }
    }

    #[injectable]
    pub struct PluginB {}
    impl PluginB {}

    impl Plugin for PluginB {
        fn name(&self) -> &'static str {
            "beta"
        }
    }

    #[injectable]
    pub struct PluginRegistry {
        #[inject]
        plugins: Vec<Arc<dyn Plugin>>,
    }
    impl PluginRegistry {}

    #[controller()]
    pub struct TestController {
        #[inject]
        registry: Arc<PluginRegistry>,
    }

    #[routes]
    impl TestController {
        #[get("/plugins")]
        fn list(&self) -> Body {
            let mut names: Vec<&str> = self.registry.plugins.iter().map(|p| p.name()).collect();
            names.sort();
            Body::text(names.join(","))
        }
    }

    #[module(
        providers: [
            provide!(into dyn Plugin => PluginA),
            provide!(into dyn Plugin => PluginB),
            PluginRegistry,
        ],
        controllers: [TestController]
    )]
    impl TestModule {}

    let server = TestServer::start(TestModule).await;
    let resp = server
        .client()
        .get(server.url("/plugins"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    // Both contributions present (order may vary, so compare sorted)
    let mut parts: Vec<&str> = body.split(',').collect();
    parts.sort();
    assert_eq!(parts, vec!["alpha", "beta"]);
}

#[tokio::test]
async fn multi_factory_closure_collects_contributions() {
    struct Greeter {
        greeting: &'static str,
    }
    impl Plugin for Greeter {
        fn name(&self) -> &'static str {
            self.greeting
        }
    }

    key!(Greeters: dyn Plugin);

    #[injectable]
    pub struct GreeterRegistry {
        #[inject(Greeters)]
        greeters: Vec<Arc<dyn Plugin>>,
    }
    impl GreeterRegistry {}

    #[controller()]
    pub struct TestController {
        #[inject]
        registry: Arc<GreeterRegistry>,
    }

    #[routes]
    impl TestController {
        #[get("/greeters")]
        fn list(&self) -> Body {
            let mut names: Vec<&str> = self.registry.greeters.iter().map(|p| p.name()).collect();
            names.sort();
            Body::text(names.join(","))
        }
    }

    #[module(
        providers: [
            provide!(into Greeters => async || Greeter { greeting: "hello" }),
            provide!(into Greeters => async || Greeter { greeting: "world" }),
            GreeterRegistry,
        ],
        controllers: [TestController]
    )]
    impl TestModule {}

    let server = TestServer::start(TestModule).await;
    let resp = server
        .client()
        .get(server.url("/greeters"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    let mut parts: Vec<&str> = body.split(',').collect();
    parts.sort();
    assert_eq!(parts, vec!["hello", "world"]);
}

#[tokio::test]
async fn a_collection_with_no_contribution_fails_startup() {
    key!(NoPlugins: dyn Plugin);

    #[injectable]
    pub struct EmptyRegistry {
        #[inject(NoPlugins)]
        plugins: Vec<Arc<dyn Plugin>>,
    }
    impl EmptyRegistry {}

    #[controller()]
    pub struct TestController {
        #[inject]
        registry: Arc<EmptyRegistry>,
    }

    #[routes]
    impl TestController {
        #[get("/count")]
        fn count(&self) -> Body {
            Body::text(self.registry.plugins.len().to_string())
        }
    }

    #[module(providers: [EmptyRegistry], controllers: [TestController])]
    impl TestModule {}

    // A collection is registered by its first contribution, so one with none has no provider.
    let refusal = match UloFactory::create(TestModule).await {
        Ok(_) => panic!("injecting a collection nothing contributes to fails startup"),
        Err(e) => e.to_string(),
    };
    assert!(refusal.contains("NoPlugins"), "{refusal}");
}

#[tokio::test]
async fn multi_single_contribution_is_vec_of_one() {
    struct Solo;
    impl Plugin for Solo {
        fn name(&self) -> &'static str {
            "solo"
        }
    }

    #[injectable]
    pub struct SingleRegistry {
        #[inject]
        plugins: Vec<Arc<dyn Plugin>>,
    }
    impl SingleRegistry {}

    #[controller()]
    pub struct TestController {
        #[inject]
        registry: Arc<SingleRegistry>,
    }

    #[routes]
    impl TestController {
        #[get("/single")]
        fn get(&self) -> Body {
            Body::text(format!(
                "count={},name={}",
                self.registry.plugins.len(),
                self.registry.plugins[0].name()
            ))
        }
    }

    #[module(
        providers: [
            provide!(into dyn Plugin => async || Solo),
            SingleRegistry,
        ],
        controllers: [TestController]
    )]
    impl TestModule {}

    let server = TestServer::start(TestModule).await;
    let resp = server
        .client()
        .get(server.url("/single"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "count=1,name=solo");
}

#[tokio::test]
async fn multi_raw_value_contributes_to_collection() {
    struct Named {
        label: &'static str,
    }
    impl Plugin for Named {
        fn name(&self) -> &'static str {
            self.label
        }
    }

    #[injectable]
    pub struct NamedRegistry {
        #[inject]
        plugins: Vec<Arc<dyn Plugin>>,
    }
    impl NamedRegistry {}

    #[controller()]
    pub struct TestController {
        #[inject]
        registry: Arc<NamedRegistry>,
    }

    #[routes]
    impl TestController {
        #[get("/named")]
        fn list(&self) -> Body {
            let mut names: Vec<&str> = self.registry.plugins.iter().map(|p| p.name()).collect();
            names.sort();
            Body::text(names.join(","))
        }
    }

    #[module(
        providers: [
            provide!(into dyn Plugin => Named { label: "foo" }),
            provide!(into dyn Plugin => Named { label: "bar" }),
            NamedRegistry,
        ],
        controllers: [TestController]
    )]
    impl TestModule {}

    let server = TestServer::start(TestModule).await;
    let resp = server
        .client()
        .get(server.url("/named"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let mut parts: Vec<&str> = resp.text().await.unwrap().leak().split(',').collect();
    parts.sort();
    assert_eq!(parts, vec!["bar", "foo"]);
}

#[tokio::test]
async fn a_contribution_from_a_type_is_built_apart_from_its_registration() {
    static BUILT: AtomicUsize = AtomicUsize::new(0);

    #[injectable]
    pub struct Alpha {}
    impl Alpha {
        #[new]
        fn new() -> Self {
            BUILT.fetch_add(1, Ordering::SeqCst);
            Self {}
        }
    }

    impl Plugin for Alpha {
        fn name(&self) -> &'static str {
            "alpha"
        }
    }

    #[injectable]
    pub struct Registry {
        #[inject]
        plugins: Vec<Arc<dyn Plugin>>,
        #[inject]
        alpha: Arc<Alpha>,
    }
    impl Registry {}

    #[controller()]
    pub struct TestController {
        #[inject]
        registry: Arc<Registry>,
    }

    #[routes]
    impl TestController {
        #[get("/built")]
        fn built(&self) -> Body {
            Body::text(format!(
                "{}:{}",
                self.registry.plugins[0].name(),
                BUILT.load(Ordering::SeqCst)
            ))
        }
    }

    #[module(
        providers: [
            Alpha,
            provide!(into dyn Plugin => Alpha),
            Registry,
        ],
        controllers: [TestController]
    )]
    impl TestModule {}

    let server = TestServer::start(TestModule).await;
    let resp = server
        .client()
        .get(server.url("/built"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.text().await.unwrap(),
        "alpha:2",
        "the plain registration and the contribution each build an `Alpha`"
    );
}
