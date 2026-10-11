//! The hyper backend as the reference host: the app on `ulo_http_hyper::ServerOn` at port 0, on
//! the listener the harness names, with h2c on so the drain's HTTP/2 shape runs against it. A
//! backend owns no host around the app, so in `Mode::Nested` it serves the app at its root, as a
//! host would after stripping the prefix.

use std::marker::PhantomData;
use std::sync::Arc;

use futures_channel::oneshot;
use ulo::app::Connected;
use ulo::{App, Signal, TaskHandle};
use ulo_http::embed::EmbedLimits;
use ulo_http_hyper::{HyperOn, ReadCount, ServerOn};

use crate::{Harness, Host, Mode, report};

/// The reference host on harness `R`'s runtime and listener: `HyperHost<OnTokio>` on tokio.
pub struct HyperHost<R: Harness> {
    pub(crate) base_url: String,
    read_count: ReadCount,
    stop: oneshot::Sender<()>,
    serving: TaskHandle,
    _harness: PhantomData<fn() -> R>,
}

impl<R: Harness> Host for HyperHost<R> {
    type Harness = R;

    async fn start(app: App<Connected>, _mode: Mode) -> Self {
        let runtime = Arc::clone(app.handle().runtime().expect("the suite's app is given a runtime"));
        let backend = HyperOn::<R::Listener>::default();
        let read_count = backend.read_count();
        let server = ServerOn::<R::Listener>::with_backend("127.0.0.1:0", backend).h2c(true);
        let app = app
            .bind(server)
            .listen()
            .await
            .unwrap_or_else(|error| crate::startup_failed!("the reference host did not listen: {}", report(&error)));
        let addr = app.addresses().first().map(|bound| bound.addr).expect("the reference host is bound");
        let (stop, stopped) = oneshot::channel::<()>();
        let serving = runtime.spawn(Box::pin(async move {
            let _ = app
                .serve(async move {
                    let _ = stopped.await;
                    Signal::new("suite")
                })
                .await;
        }));
        HyperHost { base_url: format!("http://{addr}"), read_count, stop, serving, _harness: PhantomData }
    }

    fn base_url(&self) -> String {
        self.base_url.clone()
    }

    fn limits() -> EmbedLimits {
        EmbedLimits::NONE
    }

    fn connections_read(&self) -> Option<usize> {
        Some(self.read_count.get())
    }

    async fn stop(self) {
        let _ = self.stop.send(());
        let _ = self.serving.await;
    }
}
