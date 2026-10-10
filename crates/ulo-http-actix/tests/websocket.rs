//! A gateway on the HTTP transport, bound inside actix-web, which declares `upgrades: false`: the
//! app is refused at `listen()` before the host serves anything, rather than advertising a gateway
//! whose upgrades never arrive.

use ulo::{App, Module, ModuleDef, ModuleIdentity, StartupError, injectable, routes};
use ulo_ws::WsModule;

#[injectable]
struct Echo;

#[routes]
#[ulo_ws::gateway(path = "/ws")]
impl Echo {
    #[ulo_ws::message("ping")]
    fn ping(&self) -> &'static str {
        "pong"
    }
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(WsModule::for_root());
        m.controller::<Echo>();
    }
}

#[tokio::test]
async fn a_gateway_on_the_http_transport_is_refused_by_a_host_without_upgrades() {
    let listened = App::builder(Root)
        .runtime(ulo_tokio::Tokio::current())
        .wire()
        .unwrap_or_else(|error| panic!("the app did not wire: {error}"))
        .connect()
        .await
        .unwrap_or_else(|error| panic!("the app did not connect: {error}"))
        .bind(ulo_http_actix::Embedded::new())
        .listen()
        .await;
    let Err(StartupError::Configure(errors)) = listened else {
        panic!("a gateway on the HTTP transport listened inside a host declaring `upgrades: false`");
    };
    let text = errors.to_string();
    assert!(
        text.contains("the actix embedding declares `upgrades: false`") && text.contains("serve the gateways on a separate port"),
        "the refusal does not name the host's declaration: {text}"
    );
}
