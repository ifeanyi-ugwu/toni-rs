//! The WebSocket transport (transports DESIGN §4): gateways, sessions, the connection hooks, the
//! message envelope, rooms and broadcast.
//!
//! ```ignore
//! #[module(imports = [ulo_ws::WsModule::for_root()], controllers = [ChatGateway])]
//! pub struct ChatModule;
//!
//! #[routes]
//! #[ulo_ws::gateway(path = "/chat", namespace = "lobby", session = ChatSession, connect_guards(TokenGuard))]
//! impl ChatGateway {
//!     #[ulo_ws::message("chat.send")]
//!     async fn send(&self, msg: Payload<ChatMessage>, rooms: Dep<Rooms>) -> Result<Ack, ChatError> { /* .. */ }
//! }
//! ```
//!
//! Two transports: [`Ws`] covers message handlers, one execution per message, and [`WsConnect`]
//! the connection phase, one execution per connection, whose handler `ulo-ws` mounts itself with
//! the gateway's connect guards. A gateway on the HTTP server's port is reached through the
//! upgrade hand-off [`WsModule`] registers; one declared `port = own` through [`Server`]. Both
//! run the same steps at the same moments.

mod broadcast;
mod codec;
mod connection;
mod envelope;
mod gateway;
mod handoff;
mod module;
mod rooms;
mod server;
mod session;
mod transport;

#[doc(hidden)]
pub mod __private;

pub use broadcast::{Audience, BroadcastAdapter, InMemory, NodeId, Target};
pub use codec::Codec;
pub use connection::{ConnId, Connection};
pub use envelope::{Frame, MessageId, Payload};
pub use gateway::{
    AfterInit, CloseCodeError, ConnectRefused, DisconnectReason, Gateway, GatewayConfig, GatewayRef, GatewaySettings,
    OnConnect, OnDisconnect, Overflow, Port, Refuse,
};
pub use module::WsModule;
pub use rooms::{Broadcast, BroadcastError, Rooms, RoomsIn};
pub use server::Server;
pub use session::{Session, SessionFactory, SessionHandle};
pub use transport::{ConnectCx, ConnectReply, ConnectionInfo, NoHandler, Reply, UpgradeHead, Ws, WsConnect, WsCx};
pub use ulo_ws_macros::{gateway, message};
