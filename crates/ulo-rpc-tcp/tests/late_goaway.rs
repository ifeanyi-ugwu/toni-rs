//! A connection the server accepted before its drain and registered after it, its TLS handshake
//! finishing late, still receives `goaway`. Without it the client keeps sending over a connection
//! the closing server no longer reads.
//!
//! One connection holds a call in flight, so the server stays draining. The late connection's
//! client opens its TCP connection, waits until the server has accepted it, and only once the
//! drain has begun sends its TLS `ClientHello`.

use std::error::Error;
use std::fmt;
use std::net::SocketAddr;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Notify;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use ulo::{App, Module, ModuleDef, ModuleIdentity, Signal, injectable, routes};
use ulo_net::Tls;
use ulo_net::rustls::pki_types::pem::PemObject;
use ulo_net::rustls::pki_types::{CertificateDer, ServerName};
use ulo_net::rustls::{self, ClientConfig, RootCertStore};
use ulo_rpc::{CallHeaders, Codec, Data, Frame};
use ulo_rpc_tcp::Tcp;
use ulo_transport::{Classify, ErrorKind};

const CA: &[u8] = include_bytes!("../../ulo-net/tests/fixtures/ca.pem");
const CERT: &[u8] = include_bytes!("../../ulo-net/tests/fixtures/localhost.pem");
const KEY: &[u8] = include_bytes!("../../ulo-net/tests/fixtures/localhost-key.pem");

const HOLD: &str = "late_goaway.hold";

/// How long a read waits for a frame the server writes at once.
const PATIENCE: Duration = Duration::from_secs(3);

#[derive(Debug)]
struct Never;

impl fmt::Display for Never {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("never raised")
    }
}

impl Error for Never {}

impl Classify for Never {
    fn classify(&self) -> ErrorKind {
        ErrorKind::Internal
    }
}

/// Raised by the held call once its handler runs, and by the test to let it answer.
fn running() -> &'static Notify {
    static RUNNING: OnceLock<Notify> = OnceLock::new();
    RUNNING.get_or_init(Notify::new)
}

fn release() -> &'static Notify {
    static RELEASE: OnceLock<Notify> = OnceLock::new();
    RELEASE.get_or_init(Notify::new)
}

#[injectable]
struct Holder;

#[routes]
impl Holder {
    #[ulo_rpc::message("late_goaway.hold")]
    async fn hold(&self) -> Result<u32, Never> {
        running().notify_one();
        release().notified().await;
        Ok(0)
    }
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.controller::<Holder>();
    }
}

fn connector() -> TlsConnector {
    let mut roots = RootCertStore::empty();
    roots.add(CertificateDer::from_pem_slice(CA).expect("the CA parses")).expect("the CA is a trust anchor");
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("the default versions")
        .with_root_certificates(roots)
        .with_no_client_auth();
    TlsConnector::from(Arc::new(config))
}

async fn handshake(tcp: TcpStream) -> TlsStream<TcpStream> {
    let name = ServerName::try_from("localhost").expect("a DNS name");
    connector().connect(name, tcp).await.expect("the TLS handshake with the server completes")
}

async fn write_frame<W: AsyncWrite + Unpin>(writer: &mut W, frame: &Frame) {
    let bytes = Codec::Json.encode_frame(frame).expect("the frame encodes");
    writer.write_all(&(bytes.len() as u32).to_be_bytes()).await.expect("the prefix is written");
    writer.write_all(&bytes).await.expect("the frame is written");
    writer.flush().await.expect("the frame is flushed");
}

/// The next frame, `None` at the connection's end; panics if none arrives within [`PATIENCE`].
async fn read_frame<R: AsyncRead + Unpin>(reader: &mut R, what: &str) -> Option<Frame> {
    let read = async {
        let mut prefix = [0u8; 4];
        reader.read_exact(&mut prefix).await.ok()?;
        let mut body = vec![0; u32::from_be_bytes(prefix) as usize];
        reader.read_exact(&mut body).await.ok()?;
        Some(Codec::Json.decode_frame(&body).expect("the server's frame decodes"))
    };
    tokio::time::timeout(PATIENCE, read).await.unwrap_or_else(|_| panic!("{what}: no frame within {PATIENCE:?}"))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_connection_registered_after_the_drain_began_receives_goaway() {
    let server = App::builder(Root)
        .runtime(ulo_tokio::Tokio::current())
        .wire()
        .expect("the server wires")
        .connect()
        .await
        .expect("the server connects")
        .bind(ulo_rpc::Server::new(Tcp::new("127.0.0.1:0").tls(Tls::from_pem(CERT, KEY))))
        .listen()
        .await
        .expect("the server binds");
    let addr: SocketAddr = server.addresses().first().map(|bound| bound.addr).expect("the server's address");
    let handle = server.handle();
    let serving = tokio::spawn(async move { server.serve(std::future::pending::<Signal>()).await });

    let mut holding = handshake(TcpStream::connect(addr).await.expect("the holding connection opens")).await;
    let started = running().notified();
    let hold = Frame::Req { id: 1, pattern: HOLD.to_owned(), headers: CallHeaders::new(), data: Data::new(b"null".as_slice()) };
    write_frame(&mut holding, &hold).await;
    tokio::time::timeout(PATIENCE, started).await.expect("the held call reached its handler");

    // Accepted by the server, whose connection task then waits for the `ClientHello`.
    let late = TcpStream::connect(addr).await.expect("the late connection opens");
    tokio::time::sleep(Duration::from_millis(200)).await;

    let closer = handle.clone();
    let closing = tokio::spawn(async move { closer.close(Signal::new("late goaway test")).await });
    assert_eq!(read_frame(&mut holding, "the holding connection").await, Some(Frame::Goaway), "the drain's `goaway` did not reach the holding connection");
    assert!(handle.is_draining(), "the server is not draining");

    let mut late = handshake(late).await;
    let first = read_frame(&mut late, "the late connection").await;
    assert_eq!(first, Some(Frame::Goaway), "the connection registered after the drain began received no `goaway`");

    release().notify_one();
    assert!(matches!(read_frame(&mut holding, "the held call's answer").await, Some(Frame::Res { id: 1, .. })));
    drop((holding, late));
    let _ = closing.await;
    let _ = serving.await;
}
