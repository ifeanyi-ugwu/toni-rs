//! The RPC conformance suite over the TCP link with the server speaking TLS. The client side speaks
//! none (`Tcp::tls` sets a server's certificate), so it reaches the server through the relay
//! `disrupt` cuts and then a bridge that carries each connection on over TLS, checking the
//! server's certificate against the test CA. The certificate is `ulo-net`'s test fixture.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, OnceLock};

use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tokio_rustls::TlsConnector;
use ulo::BoundAddr;
use ulo_net::Tls;
use ulo_net::rustls::pki_types::pem::PemObject;
use ulo_net::rustls::pki_types::{CertificateDer, ServerName};
use ulo_net::rustls::{self, ClientConfig, RootCertStore};
use ulo_rpc_conformance::relay::Relay;
use ulo_rpc_conformance::{Broker, startup_failed};
use ulo_rpc_tcp::Tcp;

const CA: &[u8] = include_bytes!("../../ulo-net/tests/fixtures/ca.pem");
const CERT: &[u8] = include_bytes!("../../ulo-net/tests/fixtures/localhost.pem");
const KEY: &[u8] = include_bytes!("../../ulo-net/tests/fixtures/localhost-key.pem");

struct TlsLoopback {
    relay: Relay,
    bridge: Bridge,
}

impl Broker for TlsLoopback {
    type Link = Tcp;

    /// The relay and the bridge, each on a port of its own; they learn where to forward from
    /// `client_link`, once the server is bound.
    async fn start() -> Self {
        TlsLoopback { relay: Relay::without_upstream().await, bridge: Bridge::start().await }
    }

    /// Port 0, with the test certificate.
    fn link(&self) -> Tcp {
        Tcp::new(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).tls(Tls::from_pem(CERT, KEY))
    }

    /// Through the relay, then the bridge, aimed at the address the server bound.
    fn client_link(&self, server: &[BoundAddr]) -> Tcp {
        let [bound] = server else { startup_failed!("expected one bound server address, got {server:?}") };
        assert!(bound.tls, "the server's bound address reports TLS");
        self.bridge.forward_to(bound.addr);
        self.relay.forward_to(self.bridge.addr);
        Tcp::new(self.relay.addr())
    }

    async fn disrupt(&self) {
        self.relay.cut().await;
    }

    async fn client_connections(&self) -> Option<usize> {
        Some(self.relay.open().await)
    }
}

/// Carries every plain connection made to its address on to its upstream over TLS. A connection
/// ends when either side ends it: the plain side's end becomes the TLS side's `close_notify`.
struct Bridge {
    addr: SocketAddr,
    upstream: Arc<OnceLock<SocketAddr>>,
    accepting: JoinHandle<()>,
}

impl Bridge {
    async fn start() -> Bridge {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap_or_else(|error| startup_failed!("the bridge did not bind a loopback port: {error}"));
        let addr = listener.local_addr().unwrap_or_else(|error| startup_failed!("the bridge's listener has no address: {error}"));
        let upstream = Arc::new(OnceLock::new());
        let accepting = tokio::spawn(bridge(listener, Arc::clone(&upstream), TlsConnector::from(Arc::new(client_config()))));
        Bridge { addr, upstream, accepting }
    }

    fn forward_to(&self, upstream: SocketAddr) {
        let current = *self.upstream.get_or_init(|| upstream);
        assert_eq!(current, upstream, "the bridge at {} already forwards to {current}", self.addr);
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        self.accepting.abort();
    }
}

fn client_config() -> ClientConfig {
    let mut roots = RootCertStore::empty();
    roots.add(CertificateDer::from_pem_slice(CA).expect("the CA parses")).expect("the CA is a trust anchor");
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("the default versions")
        .with_root_certificates(roots)
        .with_no_client_auth()
}

/// A connection accepted before the upstream is named is closed at once, as a refused connect
/// would fail the client's. A failed TLS handshake panics its connection's task, which closes the
/// connection and prints why.
async fn bridge(listener: TcpListener, upstream: Arc<OnceLock<SocketAddr>>, connector: TlsConnector) {
    while let Ok((mut plain, _)) = listener.accept().await {
        let Some(upstream) = upstream.get().copied() else { continue };
        let connector = connector.clone();
        tokio::spawn(async move {
            let Ok(tcp) = TcpStream::connect(upstream).await else { return };
            let name = ServerName::try_from("localhost").expect("a DNS name");
            let mut tls = match connector.connect(name, tcp).await {
                Ok(tls) => tls,
                Err(error) => panic!("the TLS handshake with the server failed: {error}"),
            };
            let _ = tokio::io::copy_bidirectional(&mut plain, &mut tls).await;
        });
    }
}

ulo_rpc_conformance::conformance_suite!(TlsLoopback);
