//! The TLS handshake under its timeout.

use std::net::SocketAddr;
use std::time::Duration;

use tokio::net::TcpStream;
use tokio_rustls::TlsAcceptor;
use ulo_http::TlsInfo;

use crate::listener::{Inner, Io};

/// The TLS handshake on `stream`, with what it settled, bounded by `timeout` when there is one. A
/// failed or timed-out handshake is routine (scanners, clients that refuse the certificate), so it
/// is logged at `debug` with the peer and the connection dropped.
pub(crate) async fn handshake(
    acceptor: &TlsAcceptor,
    stream: TcpStream,
    peer: SocketAddr,
    timeout: Option<Duration>,
) -> Option<(Io, TlsInfo)> {
    let accepted = match timeout {
        Some(timeout) => tokio::time::timeout(timeout, acceptor.accept(stream)).await,
        None => Ok(acceptor.accept(stream).await),
    };
    match accepted {
        Ok(Ok(stream)) => {
            let (_, conn) = stream.get_ref();
            let mut info = TlsInfo::new();
            if let Some(alpn) = conn.alpn_protocol() {
                info = info.alpn(alpn.to_vec());
            }
            if let Some(name) = conn.server_name() {
                info = info.server_name(name);
            }
            Some((Io(Inner::Tls(Box::new(stream))), info))
        }
        Ok(Err(error)) => {
            tracing::debug!(%peer, %error, "TLS handshake failed");
            None
        }
        Err(_) => {
            tracing::debug!(%peer, ?timeout, "TLS handshake timed out");
            None
        }
    }
}
