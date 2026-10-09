use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustls::ServerConfig;
use rustls_pki_types::pem::{self, PemObject};
use rustls_pki_types::{CertificateDer, PrivateKeyDer};

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

    /// Reads and parses the certificate and key, checks that they match, and builds the server
    /// configuration with `alpn` as the protocols offered, in preference order. The server's
    /// runtime crate builds its acceptor from it.
    pub fn load(&self, alpn: &[&[u8]]) -> Result<Arc<ServerConfig>, TlsError> {
        let (certs, key) = match &self.source {
            Source::PemFiles { cert, key } => (
                certificates(&read(cert)?).map_err(|reason| TlsError::Certificate(in_file(cert, reason)))?,
                private_key(&read(key)?).map_err(|reason| TlsError::PrivateKey(in_file(key, reason)))?,
            ),
            Source::Pem { cert, key } => (
                certificates(cert).map_err(TlsError::Certificate)?,
                private_key(key).map_err(TlsError::PrivateKey)?,
            ),
        };
        // An explicit provider rather than the process default, which panics when the build
        // enables both rustls backends and nothing installed one.
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut config = ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(TlsError::Rustls)?
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .map_err(TlsError::Rustls)?;
        config.alpn_protocols = alpn.iter().map(|protocol| protocol.to_vec()).collect();
        Ok(Arc::new(config))
    }
}

fn read(path: &Path) -> Result<Vec<u8>, TlsError> {
    fs::read(path).map_err(|source| TlsError::Read { path: path.to_owned(), source })
}

fn in_file(path: &Path, reason: String) -> String {
    format!("`{}`: {reason}", path.display())
}

fn certificates(bytes: &[u8]) -> Result<Vec<CertificateDer<'static>>, String> {
    let certs = CertificateDer::pem_slice_iter(bytes)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    if certs.is_empty() {
        return Err("no `CERTIFICATE` section found".to_owned());
    }
    Ok(certs)
}

fn private_key(bytes: &[u8]) -> Result<PrivateKeyDer<'static>, String> {
    PrivateKeyDer::from_pem_slice(bytes).map_err(|error| match error {
        pem::Error::NoItemsFound => "no private key section found".to_owned(),
        error => error.to_string(),
    })
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
