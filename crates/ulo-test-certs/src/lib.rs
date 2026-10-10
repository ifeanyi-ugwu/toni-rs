//! Certificates for TLS tests, generated when a test first asks for one: [`localhost`], a
//! self-signed certificate and key for `localhost` and `127.0.0.1` that a client trusts directly,
//! and [`Ca`], a test certificate authority for a test that needs a chain, such as a server
//! verifying its client's certificate.
//!
//! ```ignore
//! let cert = ulo_test_certs::localhost();
//! let server = Tcp::new("127.0.0.1:0").tls(Tls::from_pem(cert.cert_pem(), cert.key_pem()));
//! let client = ClientConfig::builder().with_root_certificates(cert.roots()).with_no_client_auth();
//! ```
//!
//! Nothing is read from disk, so no test depends on where a fixture sits relative to it, and a
//! certificate is valid from the moment it is made, so none expires under a suite that has not
//! changed.

use std::sync::OnceLock;

use rcgen::{BasicConstraints, CertificateParams, CertifiedIssuer, IsCa, KeyPair, KeyUsagePurpose};
use rustls::RootCertStore;
use rustls::pki_types::CertificateDer;

/// A certificate with its private key, each as PEM, the form `ulo_net::Tls::from_pem` reads.
pub struct Certified {
    cert_pem: String,
    key_pem: String,
    cert_der: CertificateDer<'static>,
}

impl Certified {
    /// A new self-signed certificate for `names`, each a DNS name or an IP address, with a key of
    /// its own.
    pub fn self_signed(names: &[&str]) -> Certified {
        let key = KeyPair::generate().expect("rcgen generates a key pair");
        let cert = params(names).self_signed(&key).expect("rcgen signs a certificate with its own key");
        Certified { cert_pem: cert.pem(), key_pem: key.serialize_pem(), cert_der: cert.der().clone() }
    }

    pub fn cert_pem(&self) -> &str {
        &self.cert_pem
    }

    /// The private key, PKCS#8.
    pub fn key_pem(&self) -> &str {
        &self.key_pem
    }

    pub fn cert_der(&self) -> &CertificateDer<'static> {
        &self.cert_der
    }

    /// A root store trusting this certificate alone, for a client checking a self-signed peer.
    pub fn roots(&self) -> RootCertStore {
        roots(&self.cert_der)
    }
}

/// One self-signed certificate for `localhost` and `127.0.0.1` per test binary, made on first use.
pub fn localhost() -> &'static Certified {
    static LOCALHOST: OnceLock<Certified> = OnceLock::new();
    LOCALHOST.get_or_init(|| Certified::self_signed(&["localhost", "127.0.0.1"]))
}

/// A test certificate authority: a self-signed CA certificate and the key it issues with.
pub struct Ca {
    issuer: CertifiedIssuer<'static, KeyPair>,
}

impl Ca {
    pub fn new() -> Ca {
        let mut params = CertificateParams::default();
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::DigitalSignature];
        let key = KeyPair::generate().expect("rcgen generates a key pair");
        Ca { issuer: CertifiedIssuer::self_signed(params, key).expect("rcgen signs the CA's certificate with its own key") }
    }

    /// A new certificate for `names` signed by this CA, with a key of its own: a server's, or a
    /// client's that a server verifies against [`roots`](Self::roots).
    pub fn issue(&self, names: &[&str]) -> Certified {
        let key = KeyPair::generate().expect("rcgen generates a key pair");
        let cert = params(names).signed_by(&key, &self.issuer).expect("rcgen signs a certificate with the CA's key");
        Certified { cert_pem: cert.pem(), key_pem: key.serialize_pem(), cert_der: cert.der().clone() }
    }

    pub fn cert_pem(&self) -> String {
        self.issuer.pem()
    }

    /// A root store trusting this CA, for a peer checking a certificate it issued.
    pub fn roots(&self) -> RootCertStore {
        roots(self.issuer.der())
    }
}

impl Default for Ca {
    fn default() -> Ca {
        Ca::new()
    }
}

/// An end-entity certificate's parameters: `names` as its subject alternative names, rcgen parsing
/// an IP address as one and anything else as a DNS name.
fn params(names: &[&str]) -> CertificateParams {
    let names: Vec<String> = names.iter().map(|name| (*name).to_owned()).collect();
    CertificateParams::new(names).expect("every name is a DNS name or an IP address")
}

fn roots(anchor: &CertificateDer<'static>) -> RootCertStore {
    let mut roots = RootCertStore::empty();
    roots.add(anchor.clone()).expect("a generated certificate is a trust anchor");
    roots
}
