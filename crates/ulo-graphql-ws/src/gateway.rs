//! The graphql-transport-ws gateway.

use std::marker::PhantomData;

use serde_json::{Map, Value};
use ulo::{Construct, ConstructError, Controller, Dependencies, Mount, Resolver, scope::Auto};
use ulo_ws::{Connection, DisconnectReason, Frame, Gateway, GatewayConfig, GatewaySettings, SessionFactory};

/// The gateway, serving the engine bound as `dyn Engine` (or `dyn Engine @ Q`): a controller listed
/// in a module's `controllers` beside an import of `WsModule`, mounted at the subscription path
/// with `ModuleDef::controller::<GraphqlWs>().at(path)`. Its settings: `event = "type"`,
/// `subprotocols = ["graphql-transport-ws"]`.
pub struct GraphqlWs<Q = ()> {
    pub(crate) _engine: PhantomData<fn() -> Q>,
}

/// The `connection_init` payload, kept in the connection's session for the context an engine
/// builds per execution: `Session<ConnectionInit>`.
#[derive(Clone, Debug, Default)]
pub struct ConnectionInit {
    pub(crate) payload: Option<Map<String, Value>>,
}

impl ConnectionInit {
    /// The payload the client sent, `None` before `connection_init` arrives or when it sent none.
    pub fn payload(&self) -> Option<&Map<String, Value>> {
        self.payload.as_ref()
    }
}

impl<Q: Send + Sync + 'static> Construct for GraphqlWs<Q> {
    type Scope = Auto;

    fn dependencies(d: &mut Dependencies) {
        let _ = d;
        todo!()
    }

    async fn construct(r: &Resolver<'_>) -> Result<Self, ConstructError> {
        let _ = r;
        todo!()
    }
}

impl<Q: Send + Sync + 'static> GatewayConfig for GraphqlWs<Q> {
    fn settings() -> GatewaySettings {
        todo!()
    }

    fn session() -> SessionFactory {
        todo!()
    }

    fn mount_gateway(m: &mut Mount<'_>) {
        <Self as Gateway>::mount(m);
    }
}

impl<Q: Send + Sync + 'static> Gateway for GraphqlWs<Q> {
    async fn on_message(&self, conn: &Connection, frame: Frame) {
        let _ = (conn, frame);
        todo!()
    }

    async fn on_disconnect(&self, conn: &Connection, why: DisconnectReason) {
        let _ = (conn, why);
        todo!()
    }
}

impl<Q: Send + Sync + 'static> Controller for GraphqlWs<Q> {
    fn mount(m: &mut Mount<'_>) {
        <Self as Gateway>::mount(m);
    }
}
