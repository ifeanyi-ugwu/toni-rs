//! Sessions: per-connection state, created before the connect guards run and kept until the last
//! execution holding it ends, `on_disconnect`'s included (transports DESIGN §4.1).

use std::any::Any;
use std::fmt;
use std::ops::Deref;
use std::sync::Arc;

use ulo::scope::{AllowedIn, Auto, PerExecution, Transient};
use ulo::{Factory, FromContainer, Key, LookupError, Requirement, Resolver};

use crate::connection::ConnId;

/// The connection's session as an execution input, seeded into the connect phase, every message
/// and `on_disconnect`, and declared by both `Ws` and `WsConnect` (X19). Code holding a
/// `Connection` reads it through `Connection::session`; a handler takes [`Session<T>`].
#[derive(Clone)]
pub struct SessionHandle {
    pub(crate) conn: ConnId,
    pub(crate) value: Option<Arc<dyn Any + Send + Sync>>,
}

impl SessionHandle {
    pub fn conn_id(&self) -> ConnId {
        self.conn
    }

    /// The session as `T`, `None` when the gateway's session is another type or it has none.
    pub fn get<T: Send + Sync + 'static>(&self) -> Option<Session<T>> {
        let _ = &self.value;
        todo!()
    }
}

impl fmt::Debug for SessionHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionHandle").field("conn", &self.conn).finish_non_exhaustive()
    }
}

/// The connection's session, typed: `session: Session<ChatSession>` in a handler, a hook or an
/// execution-scoped service. It derefs to `T`; state that changes sits behind `T`'s own
/// interior mutability. A `FromContainer` type whose `describe` declares the `SessionHandle`
/// input, so it is read wherever the connection's inputs are seeded and refused by `wire()`
/// anywhere else.
pub struct Session<T> {
    pub(crate) conn: ConnId,
    pub(crate) value: Arc<T>,
}

impl<T> Session<T> {
    /// The connection the session belongs to, for `Rooms::except([..])`.
    pub fn conn_id(&self) -> ConnId {
        self.conn
    }
}

impl<T> Clone for Session<T> {
    fn clone(&self) -> Self {
        Session { conn: self.conn, value: Arc::clone(&self.value) }
    }
}

impl<T> Deref for Session<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.value
    }
}

impl<T: Send + Sync + 'static> FromContainer for Session<T> {
    fn describe(req: &mut Requirement) {
        req.dep(Key::of::<SessionHandle, ()>());
    }

    async fn from_container(r: &Resolver<'_>) -> Result<Self, LookupError> {
        let _ = r;
        todo!()
    }
}

impl<T> AllowedIn<PerExecution> for Session<T> {}
impl<T> AllowedIn<Transient> for Session<T> {}
impl<T> AllowedIn<Auto> for Session<T> {}

/// How a gateway builds a connection's session, before the connect guards run.
pub struct SessionFactory {
    pub(crate) kind: FactoryKind,
}

pub(crate) enum FactoryKind {
    None,
    Build(Box<dyn Fn() -> Arc<dyn Any + Send + Sync> + Send + Sync>),
}

impl SessionFactory {
    /// No session: `Session<T>` fails to resolve on this gateway's connections.
    pub fn none() -> Self {
        SessionFactory { kind: FactoryKind::None }
    }

    /// `T::default()`, `session = T` on the gateway attribute.
    pub fn default_of<T: Default + Send + Sync + 'static>() -> Self {
        SessionFactory { kind: FactoryKind::Build(Box::new(|| Arc::new(T::default()))) }
    }

    /// A closure over container reads, resolved in the connection phase's execution:
    /// `session_with = |head: Dep<UpgradeHead>| ChatSession::from_token(head.headers())`.
    pub fn with<Args, F>(factory: F) -> Self
    where
        F: Factory<Args>,
        F::Output: Send + Sync + 'static,
    {
        let _ = factory;
        todo!()
    }
}
