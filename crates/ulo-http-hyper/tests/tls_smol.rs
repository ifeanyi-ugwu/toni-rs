//! The hyper backend serving TLS on smol, as `tls.rs` does on tokio: the configuration `Tls::load`
//! builds reaches `ulo-listen-smol`, which wraps each connection with `futures-rustls`; ALPN
//! settles HTTP/2 or HTTP/1.1 as the client offers; and an upgrade on a TLS connection reaches the
//! app's handler as an `Upgraded` that carries bytes both ways. The client is `futures-rustls` over
//! `async-net`, and no tokio runtime runs.

use std::future::{Future, poll_fn};
use std::net::SocketAddr;
use std::pin::pin;
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;

use async_net::TcpStream;
use futures_rustls::TlsConnector;
use futures_rustls::client::TlsStream;
use futures::{AsyncReadExt, AsyncWriteExt};
use ulo::{Runtime, Signal, TaskHandle, Timer};
use ulo_http_conformance::{Harness, OnSmol};
use ulo_http_hyper::ServerOn;
use ulo_listen_smol::SmolListener;
use ulo_net::Tls;
use ulo_net::rustls::pki_types::ServerName;
use ulo_net::rustls::{self, ClientConfig};
use ulo_test_certs::localhost;

/// How long any one step may take.
const PATIENCE: Duration = Duration::from_secs(5);

async fn within<F: Future>(timer: &dyn Timer, what: &str, fut: F) -> F::Output {
    let mut fut = pin!(fut);
    let mut expired = timer.sleep(PATIENCE);
    poll_fn(|cx| {
        if let Poll::Ready(output) = fut.as_mut().poll(cx) {
            return Poll::Ready(output);
        }
        if expired.as_mut().poll(cx).is_ready() {
            panic!("{what} did not happen within {PATIENCE:?}");
        }
        Poll::Pending
    })
    .await
}

/// The conformance suite's app, its `/echo` upgrade handler included, on the hyper backend on smol
/// with TLS at a port the OS chose.
struct Served {
    addr: SocketAddr,
    runtime: Arc<dyn Runtime>,
    stop: futures::channel::oneshot::Sender<()>,
    serving: TaskHandle,
}

impl Served {
    async fn start() -> Served {
        let cert = localhost();
        let server = ServerOn::<SmolListener>::new("127.0.0.1:0").tls(Tls::from_pem(cert.cert_pem(), cert.key_pem()));
        let app = ulo_http_conformance::app(OnSmol::runtime()).await;
        let runtime = Arc::clone(app.handle().runtime().expect("the app's runtime"));
        let app = app.bind(server).listen().await.expect("the app listens with TLS");
        let [bound] = app.addresses().try_into().expect("one bound address");
        assert!(bound.tls, "the bound address reports TLS");
        let (stop, stopped) = futures::channel::oneshot::channel::<()>();
        let serving = runtime.spawn(Box::pin(async move {
            let _ = app
                .serve(async move {
                    let _ = stopped.await;
                    Signal::new("tls")
                })
                .await;
        }));
        Served { addr: bound.addr, runtime, stop, serving }
    }

    /// A TLS connection offering `alpn`, the server's certificate trusted directly.
    async fn connect(&self, alpn: &[&[u8]]) -> TlsStream<TcpStream> {
        let roots = localhost().roots();
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut config = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("the default versions")
            .with_root_certificates(roots)
            .with_no_client_auth();
        config.alpn_protocols = alpn.iter().map(|protocol| protocol.to_vec()).collect();
        let tcp = TcpStream::connect(self.addr).await.expect("the server accepts");
        let name = ServerName::try_from("localhost").expect("a DNS name");
        within(&*self.runtime, "the TLS handshake", TlsConnector::from(Arc::new(config)).connect(name, tcp))
            .await
            .expect("the handshake succeeds")
    }

    async fn stop(self) {
        let _ = self.stop.send(());
        let _ = within(&*self.runtime, "the app's close", self.serving).await;
    }
}

/// Reads until `needle` has arrived, answering everything read.
async fn read_until(timer: &dyn Timer, stream: &mut TlsStream<TcpStream>, needle: &[u8]) -> Vec<u8> {
    let mut read = Vec::new();
    let mut buffer = [0u8; 512];
    while !read.windows(needle.len()).any(|window| window == needle) {
        let what = format!("{:?}, having read {:?},", String::from_utf8_lossy(needle), String::from_utf8_lossy(&read));
        let n = within(timer, &what, stream.read(&mut buffer)).await.expect("the read succeeds");
        assert_ne!(n, 0, "the connection ended before {:?}; read {:?}", String::from_utf8_lossy(needle), String::from_utf8_lossy(&read));
        read.extend_from_slice(&buffer[..n]);
    }
    read
}

#[test]
fn alpn_settles_the_protocol_the_client_offers() {
    OnSmol::block_on(async {
        let served = Served::start().await;
        let h2 = served.connect(&[b"h2", b"http/1.1"]).await;
        assert_eq!(h2.get_ref().1.alpn_protocol(), Some(b"h2".as_slice()), "a client offering h2 first gets h2");
        let http1 = served.connect(&[b"http/1.1"]).await;
        assert_eq!(http1.get_ref().1.alpn_protocol(), Some(b"http/1.1".as_slice()), "a client offering only HTTP/1.1 gets it");
        drop((h2, http1));
        served.stop().await;
    });
}

#[test]
fn an_upgrade_over_tls_carries_bytes_both_ways() {
    OnSmol::block_on(async {
        let served = Served::start().await;
        let timer = Arc::clone(&served.runtime);
        let mut stream = served.connect(&[b"http/1.1"]).await;
        stream
            .write_all(b"GET /echo HTTP/1.1\r\nHost: localhost\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\r\n")
            .await
            .expect("the request is written");
        let head = read_until(&*timer, &mut stream, b"\r\n\r\n").await;
        assert!(head.starts_with(b"HTTP/1.1 101"), "the server switched protocols; answered {:?}", String::from_utf8_lossy(&head));
        stream.write_all(b"ping").await.expect("the frame is written");
        stream.flush().await.expect("the frame is flushed");
        read_until(&*timer, &mut stream, b"ping").await;
        stream.write_all(b"pong").await.expect("the second frame is written");
        stream.flush().await.expect("the second frame is flushed");
        read_until(&*timer, &mut stream, b"pong").await;
        drop(stream);
        served.stop().await;
    });
}
