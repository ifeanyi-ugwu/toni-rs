//! The scenarios, one public `async fn` each, generic over the [`Host`] under test and grouped by
//! what they pin. [`ws_conformance_suite!`] stamps a `#[test]` per scenario; a host crate calls
//! one directly only to run it alone, through its own `block_on`.
//!
//! [`ws_conformance_suite!`]: crate::ws_conformance_suite

use std::sync::Arc;
use std::time::Duration;

use ulo::{App, AppHandle, BoundAddr, Runtime, Signal, TaskHandle, Timer};

use crate::app::{Probe, SuiteModule, within};
use crate::client::{Answer, Conn, Request, read_answer, write};
use crate::{DRAIN, Host, report};

pub mod close_codes;
pub mod handshake;
pub mod hooks;
pub mod keep_alive;
pub mod limits;
pub mod messages;
pub mod stream_end;

/// How often [`Served::read_past`] reads the server's count.
const COUNT_POLL: Duration = Duration::from_millis(10);

/// The suite's app, listening on the host's server and serving until the scenario stops it.
pub(crate) struct Served<H: Host> {
    addresses: Vec<BoundAddr>,
    handle: AppHandle,
    runtime: Arc<dyn Runtime>,
    timer: Arc<dyn Timer>,
    pub(crate) probe: Probe,
    serving: TaskHandle,
    host: H,
}

impl<H: Host> Served<H> {
    /// The suite's app on `H`'s runtime, bound through `H::bind`, listening and served.
    pub(crate) async fn start() -> Self {
        let probe = Probe::new();
        let app = App::builder(SuiteModule { probe: probe.clone(), port: H::PORT })
            .runtime(H::runtime())
            .drain_timeout(DRAIN)
            .wire()
            .unwrap_or_else(|error| crate::startup_failed!("the suite's app did not wire: {}", report(&error)))
            .connect()
            .await
            .unwrap_or_else(|error| crate::startup_failed!("the suite's app did not connect: {}", report(&error)));
        let (app, host) = H::bind(app);
        let app = app
            .listen()
            .await
            .unwrap_or_else(|error| crate::startup_failed!("the suite's app did not listen: {}", report(&error)));
        let addresses = app.addresses();
        if addresses.is_empty() {
            crate::startup_failed!("the suite's app listened on no address");
        }
        let handle = app.handle();
        let runtime = Arc::clone(handle.runtime().unwrap_or_else(|| crate::startup_failed!("the suite's app has no runtime")));
        let timer = Arc::clone(&runtime) as Arc<dyn Timer>;
        let serving = runtime.spawn(Box::pin(async move {
            let _ = app.serve(std::future::pending::<Signal>()).await;
        }));
        Served { addresses, handle, runtime, timer, probe, serving, host }
    }

    /// The connections the host's server reports read from, `None` where it cannot count.
    pub(crate) fn connections_read(&self) -> Option<usize> {
        self.host.connections_read()
    }

    /// Waits until the server's count of connections read from passes `counted`, read every
    /// 10 ms on the app's clock; `what` names the wait if it runs out.
    pub(crate) async fn read_past(&self, counted: usize, what: &str) {
        within(self.timer(), what, async {
            while self.connections_read().is_some_and(|now| now <= counted) {
                self.timer().sleep(COUNT_POLL).await;
            }
        })
        .await;
    }

    pub(crate) fn timer(&self) -> &dyn Timer {
        &*self.timer
    }

    /// A connection to the server, nothing written on it yet.
    pub(crate) async fn stream(&self) -> H::Stream {
        within(self.timer(), "the connection to the server", H::connect(&self.addresses))
            .await
            .unwrap_or_else(|error| panic!("connecting to {:?} failed: {error}", self.addresses))
    }

    /// The `Host` header a request to the server carries.
    pub(crate) fn host(&self) -> String {
        self.addresses[0].addr.to_string()
    }

    /// `request` sent on a connection of its own, and the server's answer as written.
    pub(crate) async fn upgrade(&self, request: &Request) -> Answer<H::Stream> {
        let mut stream = self.stream().await;
        within(self.timer(), &format!("the answer to the upgrade request for {}", request.path()), async {
            write(&mut stream, &request.head(&self.host())).await;
            read_answer(stream, request.sent_key()).await
        })
        .await
    }

    /// An upgrade to `path` carrying `headers` that the server answered 101, as a client socket.
    pub(crate) async fn connect(&self, path: &str, headers: &[(&str, &str)]) -> Conn<H::Stream> {
        let request = headers.iter().fold(Request::upgrade(path), |request, (name, value)| request.header(name, value));
        let answer = self.upgrade(&request).await;
        self.conn(answer, path)
    }

    /// The socket a 101 carries.
    pub(crate) fn conn(&self, answer: Answer<H::Stream>, path: &str) -> Conn<H::Stream> {
        match answer.socket {
            Some(ws) => Conn::new(ws, Arc::clone(&self.timer)),
            None => panic!("expected the upgrade to {path} to switch protocols, got {} {:?}", answer.status, answer.body),
        }
    }

    /// Starts the app's close on a task of its own, for a scenario that acts while the drain runs.
    pub(crate) fn close_in_background(&self) -> TaskHandle {
        let handle = self.handle.clone();
        self.runtime.spawn(Box::pin(async move {
            let _ = handle.close(Signal::new("conformance")).await;
        }))
    }

    /// Waits for a close [`close_in_background`](Self::close_in_background) started, and the serve
    /// loop's end.
    pub(crate) async fn closed(self, closing: TaskHandle) {
        let Served { timer, serving, .. } = self;
        within(&*timer, "the app's close", async {
            let _ = closing.await;
            let _ = serving.await;
        })
        .await;
    }

    /// Closes the app. A client still connected is sent 1001 and waited for until it answers or
    /// the drain runs out, so a scenario hangs up its clients first.
    pub(crate) async fn stop(self) {
        let closing = self.close_in_background();
        self.closed(closing).await;
    }
}
