//! The `poem::Endpoint` over the app: builds the app's `Request` from poem's, with the peer and the
//! upgrade `take_upgrade` yields, and answers through `Service::respond`.

use crate::Handle;

/// The endpoint a poem route mounts, from [`endpoint`].
#[derive(Clone)]
pub struct PoemEndpoint {
    pub(crate) handle: Handle,
}

/// The endpoint over `handle`, for `Route::nest(..)` or `Route::at(..)`.
pub fn endpoint(handle: &Handle) -> PoemEndpoint {
    PoemEndpoint { handle: handle.clone() }
}

impl poem::Endpoint for PoemEndpoint {
    type Output = poem::Response;

    async fn call(&self, req: poem::Request) -> poem::Result<Self::Output> {
        let _ = (req, &self.handle);
        todo!()
    }
}
