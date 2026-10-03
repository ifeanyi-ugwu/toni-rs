use std::future::Future;
use std::sync::Arc;

use ulo::{AppHandle, Timer};
use ulo_transport::Admission;

use crate::backend::HttpConfig;
use crate::pre_dispatch::Stage;
use crate::request::Request;
use crate::response::Response;
use crate::router::Router;
use crate::router::pattern::Pattern;
use crate::upgrade::UpgradeHandler;

/// What a backend calls per request: load shedding, the pre-dispatch stage, routing, `dispatch`,
/// rendering (transports DESIGN §3.7). `Clone`, an `Arc` inside, so each connection task holds its
/// own.
///
/// Per request, in order:
/// 1. Over the server's in-flight bound: 503 with `Retry-After`.
/// 2. `Execution::open` at the root module. Refused during the drain: 503 with `Retry-After` and
///    `Connection: close`, answered directly, no pre-dispatch stage.
/// 3. The inputs `RequestHead` and `ClientAddr` seeded; the `ulo_transport::span::call` span
///    entered.
/// 4. The unscoped pre-dispatch entries, in order, inside `AppHandle::catch_panic`.
/// 5. An `Upgrade` request on an upgrade path goes to its `UpgradeHandler`. Otherwise routing: a
///    miss renders 404, 405 with `Allow`, or 204 with `Allow` for `OPTIONS`, after the global error
///    handlers through `ulo::recover(None, ..)`.
/// 6. `Execution::route_to` the controller's module, the `HttpCx` built, the route's timeout
///    armed on the app's `Timer` (`CancelReason::Deadline` when it passes), the scoped entries run.
/// 7. `ulo::dispatch` with the route's call; an error no handler claims rendered as problem
///    details, `Timeout` (504) when the execution's cancel reason is `Deadline`.
/// 8. The response's headers merged with `HttpCx::response_headers`, the body wrapped so that a
///    drop before its end fires `CancelReason::Disconnected`, and a `HEAD` answered by a `GET`
///    handler stripped of its body.
#[derive(Clone)]
pub struct AppService {
    pub(crate) inner: Arc<ServiceInner>,
}

/// Everything a request reads, built once in `prepare`.
pub(crate) struct ServiceInner {
    pub(crate) app: AppHandle,
    pub(crate) router: Router,
    pub(crate) stage: Stage,
    pub(crate) upgrades: Vec<(Pattern, Arc<dyn UpgradeHandler>)>,
    pub(crate) admission: Admission,
    pub(crate) config: Arc<HttpConfig>,
    pub(crate) timer: Arc<dyn Timer>,
}

impl AppService {
    pub(crate) fn new(inner: ServiceInner) -> Self {
        AppService { inner: Arc::new(inner) }
    }

    /// Answers one request; never fails, an error being rendered as a response.
    pub fn call(&self, req: Request) -> impl Future<Output = Response> + Send + 'static {
        let inner = Arc::clone(&self.inner);
        async move {
            let _ = (inner, req);
            todo!("the pipeline above")
        }
    }
}
