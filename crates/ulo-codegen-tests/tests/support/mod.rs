//! What the gRPC tests share: an app serving `ulo_grpc::Server` on an ephemeral port, the two
//! clients a test calls it through, and a record a handler writes to and a test reads back.
//!
//! Every wait is bounded by [`WAIT`] and fails the test when it runs out.

#![allow(dead_code)]

use std::future::Future;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use tokio::sync::watch;
use tonic::transport::{Channel, Endpoint, Uri};
use ulo::{App, AppHandle, Module, Signal};

/// The longest any one wait in these tests lasts before it fails the test.
pub const WAIT: Duration = Duration::from_secs(5);

pub async fn within<F: Future>(what: &str, fut: F) -> F::Output {
    match tokio::time::timeout(WAIT, fut).await {
        Ok(output) => output,
        Err(_) => panic!("{what} did not happen within {WAIT:?}"),
    }
}

/// An app listening on one ephemeral port, serving until the test stops it.
pub struct Running {
    pub addr: SocketAddr,
    pub handle: AppHandle,
    serving: tokio::task::JoinHandle<()>,
}

impl Running {
    pub async fn start(root: impl Module, server: ulo_grpc::Server) -> Running {
        let app = App::builder(root)
            .timer(ulo_tokio::Timer)
            .drain_timeout(Duration::from_secs(4))
            .wire()
            .unwrap_or_else(|error| panic!("the gRPC app did not wire: {error}"))
            .connect()
            .await
            .unwrap_or_else(|error| panic!("the gRPC app did not connect: {error}"))
            .bind(server)
            .listen()
            .await
            .unwrap_or_else(|error| panic!("the gRPC app did not listen: {error}"));
        let addr = match app.addresses().as_slice() {
            [bound] => bound.addr,
            other => panic!("expected one bound address, got {other:?}"),
        };
        let handle = app.handle();
        let serving = tokio::spawn(async move {
            let _ = app.serve(std::future::pending::<Signal>()).await;
        });
        Running { addr, handle, serving }
    }

    pub fn origin(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// A tonic channel, connected.
    pub async fn channel(&self) -> Channel {
        within("the channel's connection", async {
            Endpoint::from_shared(self.origin())
                .unwrap_or_else(|error| panic!("bad endpoint: {error}"))
                .connect()
                .await
                .unwrap_or_else(|error| panic!("the client did not connect to {}: {error}", self.addr))
        })
        .await
    }

    /// An HTTP/2 client with no clock of its own, and the origin to build a generated client over
    /// it with: `ClockClient::with_origin(client, origin)`.
    ///
    /// A tonic `Channel` enforces a request's `grpc-timeout` itself and answers CANCELLED when it
    /// passes, which would race the server's own answer to the same deadline.
    pub fn plain_http2(&self) -> (PlainHttp2, Uri) {
        let client = Client::builder(TokioExecutor::new()).http2_only(true).build_http::<tonic::body::Body>();
        let origin: Uri = self.origin().parse().unwrap_or_else(|error| panic!("bad origin: {error}"));
        (client, origin)
    }

    pub async fn stop(self) {
        within("the app's close", async {
            let _ = self.handle.close(Signal::new("test")).await;
            let _ = self.serving.await;
        })
        .await;
    }
}

pub type PlainHttp2 = Client<HttpConnector, tonic::body::Body>;

/// Items a handler writes while a test runs, which the test reads back or waits for.
pub struct Record<T> {
    items: Arc<Mutex<Vec<T>>>,
    count: Arc<watch::Sender<usize>>,
}

impl<T> Clone for Record<T> {
    fn clone(&self) -> Self {
        Record { items: Arc::clone(&self.items), count: Arc::clone(&self.count) }
    }
}

impl<T: Clone> Record<T> {
    pub fn new() -> Self {
        Record { items: Arc::new(Mutex::new(Vec::new())), count: Arc::new(watch::channel(0).0) }
    }

    pub fn push(&self, item: T) {
        let mut items = self.items.lock().unwrap_or_else(PoisonError::into_inner);
        items.push(item);
        let len = items.len();
        drop(items);
        self.count.send_replace(len);
    }

    pub fn snapshot(&self) -> Vec<T> {
        self.items.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    pub async fn at_least(&self, n: usize, what: &str) -> Vec<T> {
        let mut count = self.count.subscribe();
        within(what, count.wait_for(|len| *len >= n)).await.unwrap_or_else(|_| panic!("the record of {what} closed"));
        self.snapshot()
    }
}
