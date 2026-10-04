//! `ulo_rpc::Server<L>`: the core's `Server` for [`Rpc`] over one link (transports DESIGN §5.3).

use ulo::{BoundAddr, BoxError, DrainToken, Mounted};
use ulo_transport::Count;

use crate::link::Link;
use crate::transport::Rpc;

/// The RPC server over link `L`: `app.bind(ulo_rpc::Server::new(ulo_rpc_tcp::Tcp::new("0.0.0.0:7000")))`.
///
/// `prepare` calls `Link::prepare`, refuses two handlers for one pattern, a handler whose shape the
/// link's capabilities do not carry (a streamed shape on UDP), a `Binary` payload on a link
/// declaring `binary: false`, and `max_inflight(Count::Max(0))`. `bind` calls `Link::listen` with
/// every mounted pattern. Over the in-flight limit a call is answered `err` of kind `unavailable`.
pub struct Server<L: Link> {
    pub(crate) link: L,
    pub(crate) max_inflight: Count,
}

impl<L: Link> Server<L> {
    pub fn new(link: L) -> Self {
        Server { link, max_inflight: Count::Default }
    }

    /// Calls in flight at once: unbounded at `Count::Default`; over it a call is refused
    /// `unavailable`. `Count::Max(0)` is refused in `prepare`. On AMQP the per-consumer prefetch
    /// follows it, 64 under `Default` or `Unlimited`.
    pub fn max_inflight(mut self, calls: Count) -> Self {
        self.max_inflight = calls;
        self
    }
}

impl<L: Link> ulo::Server for Server<L> {
    type Transport = Rpc;

    async fn prepare(&mut self, mounted: Mounted<'_, Rpc>) -> Result<(), BoxError> {
        let _ = mounted;
        todo!()
    }

    async fn bind(&mut self, mounted: Mounted<'_, Rpc>) -> Result<(), BoxError> {
        let _ = mounted;
        todo!()
    }

    async fn serve(&self) -> Result<(), BoxError> {
        todo!()
    }

    async fn drain(&self, token: DrainToken) {
        let _ = token;
        todo!()
    }

    async fn close(&self) -> Result<(), BoxError> {
        todo!()
    }

    /// What the link reports, so a TCP or UDP server on port 0 shows its port.
    fn bound(&self) -> Vec<BoundAddr> {
        self.link.bound()
    }
}
