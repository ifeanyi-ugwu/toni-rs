use crate::context::{Extensions, WsContext};
use crate::extractors::FromContext;

/// State that outlives the executions on one connection.
///
/// A message is an execution and its bag empties with it. What a connect guard establishes — the
/// principal it authenticated, the tenant it resolved — belongs to the connection instead, and this
/// is where it lives.
///
/// A store rather than a context: it carries values and a lifetime, and none of what a
/// [`WsContext`] carries besides. It is created before the connect guards run and dropped when the
/// client goes, so a value left here is dropped with the connection and a reconnect starts empty.
/// This is a connection's lifetime, not a user's.
///
/// Read it through [`WsContext::session`], or take it as a handler parameter:
///
/// ```rust,ignore
/// #[subscribe_message("orders")]
/// async fn orders(&self, session: Session, Payload(order): Payload<Order>) -> WsHandlerResult {
///     let who = session.get::<Principal>().ok_or(WsError::AuthFailed("no principal".into()))?;
///     // ...
/// }
/// ```
///
/// Distinct from [`Extensions`] on purpose. The two hold the same kind of thing at different
/// lifetimes, and a bare `Extensions` in a signature says nothing about which one it is.
#[derive(Clone, Default)]
pub struct Session {
    bag: Extensions,
}

impl Session {
    pub fn new() -> Self {
        Self {
            bag: Extensions::new(),
        }
    }

    /// The value stored under `T`, cloned, or `None` if nothing wrote one.
    pub fn get<T: Clone + Send + Sync + 'static>(&self) -> Option<T> {
        self.bag.get::<T>()
    }

    /// Read `T` in place, for a payload that does not clone.
    pub fn with<T: Send + Sync + 'static, R>(&self, f: impl FnOnce(&T) -> R) -> Option<R> {
        self.bag.with::<T, R>(f)
    }

    /// Store `T` for the rest of the connection, returning whatever it replaced.
    pub fn insert<T: Send + Sync + 'static>(&self, value: T) -> Option<T> {
        self.bag.insert(value)
    }

    /// Read and mutate `T` in place.
    pub fn with_mut<T: Send + Sync + 'static, R>(&self, f: impl FnOnce(&mut T) -> R) -> Option<R> {
        self.bag.with_mut::<T, R>(f)
    }

    pub fn remove<T: Send + Sync + 'static>(&self) -> Option<T> {
        self.bag.remove::<T>()
    }

    pub fn contains<T: Send + Sync + 'static>(&self) -> bool {
        self.bag.contains::<T>()
    }
}

impl FromContext<WsContext> for Session {
    type Error = std::convert::Infallible;

    async fn extract(ctx: &WsContext) -> Result<Self, Self::Error> {
        Ok(ctx.session().clone())
    }
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session").finish_non_exhaustive()
    }
}
