mod nats_adapter;
mod nats_client_transport;
mod servers;

/// The subject carrying stream-cancel notices (ADR-0032). Every server
/// instance subscribes without a queue group, so each sees every notice and
/// only the instance holding the call acts on it.
pub(crate) const CANCEL_SUBJECT: &str = "toni.rpc.cancel";

pub use nats_adapter::NatsAdapter;
pub use nats_client_transport::NatsClientTransport;
pub use servers::IntoNatsServers;
pub use toni::{RpcAdapter, RpcClient, RpcClientTransport};
