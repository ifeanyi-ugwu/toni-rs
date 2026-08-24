//! A serve loop that returns on its own must bring the whole application down.
//!
//! An adapter stops accepting only when it is shutting down or when its transport has failed.
//! In the second case a process that keeps serving its remaining transports answers less than
//! it advertises, while every liveness signal still reads healthy.

use std::sync::Arc;

use anyhow::Result;
use tokio::sync::oneshot;
use toni::http_helpers::HttpMethod;
use toni::{
    async_trait, controller, get, module, routes, AdapterContext, BindTarget, Body as ToniBody,
    HttpAdapter, HttpLifecycleHandle, RequestHandler, ToniFactory,
};

#[controller("/probe")]
pub struct ProbeController {}

#[routes]
impl ProbeController {
    #[get("/ping")]
    fn ping(&self) -> ToniBody {
        ToniBody::text("pong")
    }
}

#[module(controllers: [ProbeController])]
impl ProbeModule {}

/// Binds a real socket so the framework sees an ordinary bound adapter, then serves nothing:
/// the loop simply waits to be told to return, standing in for a transport that dies.
struct DyingAdapter {
    die: oneshot::Receiver<()>,
}

#[async_trait]
impl HttpAdapter for DyingAdapter {
    fn register_route(
        &mut self,
        _method: HttpMethod,
        _path: &str,
        _handler: Arc<dyn RequestHandler>,
    ) -> Result<()> {
        Ok(())
    }

    async fn into_lifecycle(
        self: Box<Self>,
        target: BindTarget,
        _ctx: AdapterContext,
    ) -> Result<HttpLifecycleHandle> {
        let listener = target.into_std_listener()?;
        let local_addr = listener.local_addr()?;
        let die = self.die;

        let serve = Box::pin(async move {
            // Holding the listener keeps the port occupied for as long as the loop runs.
            let _listener = listener;
            let _ = die.await;
        });

        Ok(HttpLifecycleHandle::new(local_addr, serve, || async {
            Ok(())
        }))
    }
}

#[tokio_localset_test::localset_test]
async fn a_dead_serve_loop_closes_the_application() {
    let (die, dies) = oneshot::channel();

    let mut app = ToniFactory::create(ProbeModule).await.unwrap();
    app.use_http_adapter(DyingAdapter { die: dies }, ("127.0.0.1", 0))
        .unwrap();
    app.bind().await.unwrap();

    let shutdown = app.shutdown_handle();
    let run = tokio::task::spawn_local(async move { app.run().await });

    die.send(()).unwrap();

    // Nothing signals shutdown here — the serve loop returning is the only trigger.
    tokio::time::timeout(std::time::Duration::from_secs(5), run)
        .await
        .expect("run() should return once the serve loop died")
        .unwrap();

    assert!(
        shutdown.is_shutdown(),
        "a dead serve loop should have signalled application shutdown"
    );
}
