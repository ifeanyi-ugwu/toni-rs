//! A controller's `Vec<Arc<dyn Trait>>` field resolves the collection's contributions, the same ones
//! a provider declaring the identical field receives, whether the controller is built once or per
//! call, and so does a `#[new]` parameter of that type.

use std::sync::Arc;

use ulo::http::Body;
use ulo::{controller, get, injectable, module, new, provide, routes};

use crate::common::TestServer;

pub trait Plugin: Send + Sync {
    fn name(&self) -> &'static str;
}

pub struct Alpha;

impl Plugin for Alpha {
    fn name(&self) -> &'static str {
        "alpha"
    }
}

pub struct Beta;

impl Plugin for Beta {
    fn name(&self) -> &'static str {
        "beta"
    }
}

fn names(plugins: &[Arc<dyn Plugin>]) -> String {
    plugins
        .iter()
        .map(|plugin| plugin.name())
        .collect::<Vec<_>>()
        .join(",")
}

#[injectable]
pub struct Registry {
    #[inject]
    plugins: Vec<Arc<dyn Plugin>>,
}

#[injectable]
pub struct Built {
    plugins: Vec<Arc<dyn Plugin>>,
}

impl Built {
    #[new]
    fn new(plugins: Vec<Arc<dyn Plugin>>) -> Self {
        Self { plugins }
    }
}

#[controller("/plugins")]
pub struct Plugins {
    #[inject]
    plugins: Vec<Arc<dyn Plugin>>,
    #[inject]
    registry: Registry,
    #[inject]
    built: Built,
}

#[routes]
impl Plugins {
    #[get("/")]
    fn list(&self) -> Body {
        Body::text(format!(
            "{}|{}|{}",
            names(&self.plugins),
            names(&self.registry.plugins),
            names(&self.built.plugins)
        ))
    }
}

#[controller("/per-call-plugins", scope = "execution")]
pub struct PerCallPlugins {
    #[inject]
    plugins: Vec<Arc<dyn Plugin>>,
}

#[routes]
impl PerCallPlugins {
    #[get("/")]
    fn list(&self) -> Body {
        Body::text(names(&self.plugins))
    }
}

#[module(
    controllers: [Plugins, PerCallPlugins],
    providers: [
        provide!(into dyn Plugin => value Alpha),
        provide!(into dyn Plugin => value Beta),
        Registry,
        Built,
    ],
)]
struct PluginModule;

async fn body(server: &TestServer, path: &str) -> String {
    server
        .client()
        .get(server.url(path))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap()
}

#[tokio::test]
async fn a_controller_and_a_provider_read_the_same_collection() {
    let server = TestServer::start(PluginModule).await;

    assert_eq!(
        body(&server, "/plugins").await,
        "alpha,beta|alpha,beta|alpha,beta"
    );
    assert_eq!(body(&server, "/per-call-plugins").await, "alpha,beta");
}
