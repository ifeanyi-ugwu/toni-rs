use std::error::Error;
use std::fmt;
use std::io;
use std::path::PathBuf;

use tokio_rustls::TlsAcceptor;

/// A server's TLS certificate chain and private key, loaded with rustls.
///
/// Nothing is read or parsed where a `Tls` is built: the server calls [`load`](Self::load) in its
/// `prepare`, so an unreadable file, a bad certificate or a key that does not match it fails
/// startup as `StartupError::Configure` beside every other configuration error, never inside the
/// serve loop. ALPN is set per transport at load: `h2` and `http/1.1` for HTTP, `h2` for gRPC.
#[derive(Clone)]
pub struct Tls {
    source: Source,
}

#[derive(Clone)]
enum Source {
    PemFiles { cert: PathBuf, key: PathBuf },
    Pem { cert: Vec<u8>, key: Vec<u8> },
}

impl Tls {
    /// PEM files: the certificate chain, leaf first, and its private key.
    pub fn from_pem_files(cert: impl Into<PathBuf>, key: impl Into<PathBuf>) -> Tls {
        Tls { source: Source::PemFiles { cert: cert.into(), key: key.into() } }
    }

    /// PEM bytes already in memory.
    pub fn from_pem(cert: impl Into<Vec<u8>>, key: impl Into<Vec<u8>>) -> Tls {
        Tls { source: Source::Pem { cert: cert.into(), key: key.into() } }
    }

    /// Reads and parses the certificate and key, checks that they match, and builds the acceptor
    /// with `alpn` as the protocols offered, in preference order.
    pub fn load(&self, alpn: &[&[u8]]) -> Result<TlsAcceptor, TlsError> {
        let _ = (alpn, &self.source);
        todo!("read the PEM, parse with `rustls_pki_types::pem`, build a `ServerConfig` with the ring provider, set ALPN")
    }
}

/// Why a server's TLS configuration could not be loaded.
#[non_exhaustive]
#[derive(Debug)]
pub enum TlsError {
    Read { path: PathBuf, source: io::Error },
    /// The certificate PEM holds no certificate, or one that does not parse.
    Certificate(String),
    /// The key PEM holds no private key, or one that does not parse.
    PrivateKey(String),
    /// rustls refused the pair: a key that does not match the certificate, an unsupported key type.
    Rustls(rustls::Error),
}

impl fmt::Display for TlsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TlsError::Read { path, source } => write!(f, "cannot read `{}`: {source}", path.display()),
            TlsError::Certificate(reason) => write!(f, "bad TLS certificate: {reason}"),
            TlsError::PrivateKey(reason) => write!(f, "bad TLS private key: {reason}"),
            TlsError::Rustls(error) => write!(f, "TLS configuration refused: {error}"),
        }
    }
}

impl Error for TlsError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            TlsError::Read { source, .. } => Some(source),
            TlsError::Rustls(error) => Some(error),
            _ => None,
        }
    }
}
