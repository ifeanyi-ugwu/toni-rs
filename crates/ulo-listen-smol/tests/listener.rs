//! The smol listener through `ulo-hyper-serve`'s accept loop, on a smol executor with no tokio
//! runtime: a socket on port 0 adopted, a connection reported with both its addresses, and a TLS
//! connection through `futures-rustls` reporting the ALPN and SNI its session settled.

use std::future::{Future, poll_fn};
use std::net::SocketAddr;
use std::pin::pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::Poll;
use std::time::Duration;

use async_net::TcpStream;
use futures::FutureExt;
use futures_rustls::TlsConnector;
use ulo::{Runtime, Timer};
use ulo_http::ConnInfo;
use ulo_hyper_serve::{Accepted, Serve, ServeConfig};
use ulo_listen_smol::SmolListener;
use ulo_net::rustls::pki_types::ServerName;
use ulo_net::rustls::{self, ClientConfig};
use ulo_net::{BoundListener, Endpoint, Tls};
use ulo_smol::{Executor, Smol};
use ulo_test_certs::localhost;

const PATIENCE: Duration = Duration::from_secs(5);

/// Runs `test` on a `Smol` runtime over an executor this thread and one more run.
fn on_smol<F, Fut>(test: F) -> Fut::Output
where
    F: FnOnce(Arc<dyn Runtime>) -> Fut,
    Fut: Future,
{
    let executor = Arc::new(Executor::new());
    let (stop, stopped) = futures::channel::oneshot::channel::<()>();
    let worker = {
        let executor = Arc::clone(&executor);
        std::thread::spawn(move || async_io::block_on(executor.run(stopped.map(|_| ()))))
    };
    let output = async_io::block_on(executor.run(test(Arc::new(Smol::new(Arc::clone(&executor))))));
    drop(stop);
    worker.join().expect("the executor's second thread panicked");
    output
}

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

/// A server accepting on port 0 through the smol listener, recording each connection it is handed
/// and holding it open until the drain.
struct Recording {
    addr: SocketAddr,
    runtime: Arc<dyn Runtime>,
    serve: Arc<Serve<SmolListener>>,
    seen: Arc<Mutex<Vec<(ConnInfo, bool)>>>,
}

impl Recording {
    fn start(runtime: &Arc<dyn Runtime>, tls: Option<Tls>) -> Recording {
        let listener = BoundListener::bind(&Endpoint::parse("127.0.0.1:0").expect("an address")).expect("a port");
        let addr = listener.local_addr();
        assert_ne!(addr.port(), 0, "port 0 reports the port the OS chose");
        let tls = tls.map(|tls| tls.load(&[b"h2".as_slice(), b"http/1.1".as_slice()]).expect("the certificate loads"));
        let serve = Arc::new(Serve::new(vec![listener], tls, &ServeConfig::default(), Arc::clone(runtime)).expect("the smol listener adopts the socket"));
        let seen: Arc<Mutex<Vec<(ConnInfo, bool)>>> = Arc::default();
        let (serving, recorded) = (Arc::clone(&serve), Arc::clone(&seen));
        drop(runtime.spawn(Box::pin(async move {
            let _ = serving
                .run(move |accepted: Accepted<SmolListener>| {
                    let tls = accepted.io.get_ref().get_ref().is_tls();
                    recorded.lock().unwrap_or_else(PoisonError::into_inner).push((accepted.conn.clone(), tls));
                    async move {
                        let _io = accepted.io;
                        accepted.draining.wait().await;
                    }
                })
                .await;
        })));
        Recording { addr, runtime: Arc::clone(runtime), serve, seen }
    }

    async fn first(&self) -> (ConnInfo, bool) {
        within(&*self.runtime, "the connection reaching the server", async {
            loop {
                if let Some(first) = self.seen.lock().unwrap_or_else(PoisonError::into_inner).first().cloned() {
                    return first;
                }
                self.runtime.sleep(Duration::from_millis(5)).await;
            }
        })
        .await
    }

    async fn stop(self) {
        within(&*self.runtime, "the drain", self.serve.drain()).await;
    }
}

#[test]
fn a_connection_on_port_0_is_reported_with_both_its_addresses() {
    on_smol(|runtime| async move {
        let server = Recording::start(&runtime, None);
        let stream = TcpStream::connect(server.addr).await.expect("the listener accepts");
        let (conn, tls) = server.first().await;
        assert_eq!(conn.peer, Some(stream.local_addr().expect("the client's address")));
        assert_eq!(conn.local, Some(server.addr));
        assert!(conn.tls.is_none() && !tls, "a plain connection reports no TLS");
        drop(stream);
        server.stop().await;
    });
}

#[test]
fn a_tls_connection_reports_the_alpn_and_sni_its_session_settled() {
    on_smol(|runtime| async move {
        let cert = localhost();
        let server = Recording::start(&runtime, Some(Tls::from_pem(cert.cert_pem(), cert.key_pem())));
        let mut config = ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .expect("the default versions")
            .with_root_certificates(cert.roots())
            .with_no_client_auth();
        config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        let tcp = TcpStream::connect(server.addr).await.expect("the listener accepts");
        let name = ServerName::try_from("localhost").expect("a DNS name");
        let stream = within(&*runtime, "the TLS handshake", TlsConnector::from(Arc::new(config)).connect(name, tcp))
            .await
            .expect("the handshake succeeds");
        let (conn, tls) = server.first().await;
        assert!(tls, "the connection the server was handed completed a TLS handshake");
        let info = conn.tls.expect("the connection reports its TLS");
        assert_eq!(info.alpn.as_deref(), Some(b"h2".as_slice()), "the protocol ALPN settled");
        assert_eq!(info.server_name.as_deref(), Some("localhost"), "the name the client sent");
        assert_eq!(conn.version, http::Version::HTTP_2, "ALPN's h2 makes the connection HTTP/2");
        drop(stream);
        server.stop().await;
    });
}
