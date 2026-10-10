//! The hyper backend serving TLS: the configuration `Tls::load` builds reaches the shared accept
//! loop, which wraps each connection with `tokio-rustls`; ALPN settles HTTP/2 or HTTP/1.1 as the
//! client offers; and an upgrade on a TLS connection reaches the app's handler as an `Upgraded`
//! that carries bytes both ways. The certificate is `ulo-test-certs`' self-signed one for
//! `localhost`, made when the tests run.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use ulo::Signal;
use ulo_net::Tls;
use ulo_net::rustls::pki_types::ServerName;
use ulo_net::rustls::{self, ClientConfig};
use ulo_test_certs::localhost;

/// How long any one step may take.
const PATIENCE: Duration = Duration::from_secs(5);

/// The conformance suite's app, its `/echo` upgrade handler included, on the hyper backend with
/// TLS at a port the OS chose.
struct Served {
    addr: SocketAddr,
    stop: oneshot::Sender<()>,
    serving: JoinHandle<()>,
}

impl Served {
    async fn start() -> Served {
        let cert = localhost();
        let server = ulo_http_hyper::Server::new("127.0.0.1:0").tls(Tls::from_pem(cert.cert_pem(), cert.key_pem()));
        let app = ulo_http_conformance::app().await.bind(server).listen().await.expect("the app listens with TLS");
        let [bound] = app.addresses().try_into().expect("one bound address");
        assert!(bound.tls, "the bound address reports TLS");
        let (stop, stopped) = oneshot::channel::<()>();
        let serving = tokio::spawn(async move {
            let _ = app
                .serve(async move {
                    let _ = stopped.await;
                    Signal::new("tls")
                })
                .await;
        });
        Served { addr: bound.addr, stop, serving }
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
        tokio::time::timeout(PATIENCE, TlsConnector::from(Arc::new(config)).connect(name, tcp))
            .await
            .expect("the handshake ends in time")
            .expect("the handshake succeeds")
    }

    async fn stop(self) {
        let _ = self.stop.send(());
        let _ = self.serving.await;
    }
}

/// Reads until `needle` has arrived, answering everything read.
async fn read_until(stream: &mut TlsStream<TcpStream>, needle: &[u8]) -> Vec<u8> {
    let mut read = Vec::new();
    let mut buffer = [0u8; 512];
    while !read.windows(needle.len()).any(|window| window == needle) {
        let n = tokio::time::timeout(PATIENCE, stream.read(&mut buffer))
            .await
            .unwrap_or_else(|_| {
                panic!("{:?} did not arrive in time; read {:?}", String::from_utf8_lossy(needle), String::from_utf8_lossy(&read))
            })
            .expect("the read succeeds");
        assert_ne!(n, 0, "the connection ended before {:?}; read {:?}", String::from_utf8_lossy(needle), String::from_utf8_lossy(&read));
        read.extend_from_slice(&buffer[..n]);
    }
    read
}

#[tokio::test(flavor = "multi_thread")]
async fn alpn_settles_the_protocol_the_client_offers() {
    let served = Served::start().await;
    let h2 = served.connect(&[b"h2", b"http/1.1"]).await;
    assert_eq!(h2.get_ref().1.alpn_protocol(), Some(b"h2".as_slice()), "a client offering h2 first gets h2");
    let http1 = served.connect(&[b"http/1.1"]).await;
    assert_eq!(http1.get_ref().1.alpn_protocol(), Some(b"http/1.1".as_slice()), "a client offering only HTTP/1.1 gets it");
    drop((h2, http1));
    served.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_upgrade_over_tls_carries_bytes_both_ways() {
    let served = Served::start().await;
    let mut stream = served.connect(&[b"http/1.1"]).await;
    stream
        .write_all(b"GET /echo HTTP/1.1\r\nHost: localhost\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\r\n")
        .await
        .expect("the request is written");
    let head = read_until(&mut stream, b"\r\n\r\n").await;
    assert!(head.starts_with(b"HTTP/1.1 101"), "the server switched protocols; answered {:?}", String::from_utf8_lossy(&head));
    stream.write_all(b"ping").await.expect("the frame is written");
    read_until(&mut stream, b"ping").await;
    stream.write_all(b"pong").await.expect("the second frame is written");
    read_until(&mut stream, b"pong").await;
    drop(stream);
    served.stop().await;
}
