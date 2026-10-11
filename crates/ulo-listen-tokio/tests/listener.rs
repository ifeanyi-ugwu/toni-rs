//! The tokio listener through `ulo-hyper-serve`'s accept loop: a socket on port 0 adopted, a
//! connection reported with both its addresses, a TLS connection's ALPN and SNI read off its
//! session, and adopting outside a tokio runtime refused with the listener named rather than a
//! panic.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use ulo::Runtime;
use ulo_http::ConnInfo;
use ulo_hyper_serve::{Accepted, Listener, Serve, ServeConfig};
use ulo_listen_tokio::TokioListener;
use ulo_net::rustls::pki_types::ServerName;
use ulo_net::rustls::{self, ClientConfig};
use ulo_net::{BoundListener, Endpoint, Tls};
use ulo_test_certs::localhost;
use ulo_tokio::Tokio;

const PATIENCE: Duration = Duration::from_secs(5);

/// A server accepting on port 0 through the tokio listener, recording each connection it is handed
/// and holding it open until the drain.
struct Recording {
    addr: SocketAddr,
    serve: Arc<Serve<TokioListener>>,
    seen: Arc<Mutex<Vec<(ConnInfo, bool)>>>,
}

impl Recording {
    fn start(tls: Option<Tls>) -> Recording {
        let listener = BoundListener::bind(&Endpoint::parse("127.0.0.1:0").expect("an address")).expect("a port");
        let addr = listener.local_addr();
        assert_ne!(addr.port(), 0, "port 0 reports the port the OS chose");
        let tls = tls.map(|tls| tls.load(&[b"h2".as_slice(), b"http/1.1".as_slice()]).expect("the certificate loads"));
        let runtime: Arc<dyn Runtime> = Arc::new(Tokio::current());
        let serve = Arc::new(Serve::new(vec![listener], tls, &ServeConfig::default(), runtime).expect("the tokio listener adopts the socket"));
        let seen: Arc<Mutex<Vec<(ConnInfo, bool)>>> = Arc::default();
        let (serving, recorded) = (Arc::clone(&serve), Arc::clone(&seen));
        tokio::spawn(async move {
            let _ = serving
                .run(move |accepted: Accepted<TokioListener>| {
                    let tls = accepted.io.get_ref().inner().is_tls();
                    recorded.lock().unwrap_or_else(PoisonError::into_inner).push((accepted.conn.clone(), tls));
                    async move {
                        let _io = accepted.io;
                        accepted.draining.wait().await;
                    }
                })
                .await;
        });
        Recording { addr, serve, seen }
    }

    async fn first(&self) -> (ConnInfo, bool) {
        tokio::time::timeout(PATIENCE, async {
            loop {
                if let Some(first) = self.seen.lock().unwrap_or_else(PoisonError::into_inner).first().cloned() {
                    return first;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the connection reached the server")
    }

    async fn stop(self) {
        tokio::time::timeout(PATIENCE, self.serve.drain()).await.expect("the drain ends");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_connection_on_port_0_is_reported_with_both_its_addresses() {
    let server = Recording::start(None);
    let stream = TcpStream::connect(server.addr).await.expect("the listener accepts");
    let (conn, tls) = server.first().await;
    assert_eq!(conn.peer, Some(stream.local_addr().expect("the client's address")));
    assert_eq!(conn.local, Some(server.addr));
    assert!(conn.tls.is_none() && !tls, "a plain connection reports no TLS");
    drop(stream);
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tls_connection_reports_the_alpn_and_sni_its_session_settled() {
    let cert = localhost();
    let server = Recording::start(Some(Tls::from_pem(cert.cert_pem(), cert.key_pem())));
    let mut config = ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .expect("the default versions")
        .with_root_certificates(cert.roots())
        .with_no_client_auth();
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    let tcp = TcpStream::connect(server.addr).await.expect("the listener accepts");
    let name = ServerName::try_from("localhost").expect("a DNS name");
    let stream = tokio::time::timeout(PATIENCE, TlsConnector::from(Arc::new(config)).connect(name, tcp))
        .await
        .expect("the handshake ends in time")
        .expect("the handshake succeeds");
    let (conn, tls) = server.first().await;
    assert!(tls, "the connection the server was handed completed a TLS handshake");
    let info = conn.tls.expect("the connection reports its TLS");
    assert_eq!(info.alpn.as_deref(), Some(b"h2".as_slice()), "the protocol ALPN settled");
    assert_eq!(info.server_name.as_deref(), Some("localhost"), "the name the client sent");
    assert_eq!(conn.version, http::Version::HTTP_2, "ALPN's h2 makes the connection HTTP/2");
    drop(stream);
    server.stop().await;
}

#[test]
fn adopting_outside_a_tokio_runtime_is_refused_naming_the_listener() {
    let listener = BoundListener::bind(&Endpoint::parse("127.0.0.1:0").expect("an address")).expect("a port");
    let Err(error) = TokioListener::adopt(listener, None) else {
        panic!("the tokio listener adopted a socket with no tokio runtime running");
    };
    let text = error.to_string();
    assert!(text.contains("tokio listener") && text.contains("no tokio runtime"), "the refusal names neither: {text}");
}
