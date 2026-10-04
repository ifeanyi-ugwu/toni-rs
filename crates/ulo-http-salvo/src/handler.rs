//! The `salvo::Handler` over the app: builds the app's `Request` from salvo's, prefix stripped,
//! with the peer and the upgrade future, and answers through `Service::respond`.

use crate::Handle;

/// The handler a salvo router mounts, from [`handler`].
#[derive(Clone)]
pub struct SalvoHandler {
    pub(crate) handle: Handle,
}

/// The handler over `handle`, for `Router::goal(..)`.
pub fn handler(handle: &Handle) -> SalvoHandler {
    SalvoHandler { handle: handle.clone() }
}

#[salvo::async_trait]
impl salvo::Handler for SalvoHandler {
    async fn handle(
        &self,
        req: &mut salvo::Request,
        depot: &mut salvo::Depot,
        res: &mut salvo::Response,
        ctrl: &mut salvo::FlowCtrl,
    ) {
        let _ = (req, depot, res, ctrl, &self.handle);
        todo!()
    }
}
