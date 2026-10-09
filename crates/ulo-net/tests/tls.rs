//! `Tls::load` answers a rustls `ServerConfig` that completes a handshake with no runtime at all:
//! the two rustls connections below exchange their records through plain buffers, so a backend on
//! any runtime gets TLS by wrapping the same configuration in its own TLS crate.
//!
//! `tests/fixtures` holds a test CA (`ca.pem`) and a certificate it signed for `localhost` and
//! `127.0.0.1` (`localhost.pem`, key `localhost-key.pem`), both valid until 2126.

use std::io::{Read, Write};
use std::sync::Arc;

use ulo_net::rustls::pki_types::pem::PemObject;
use ulo_net::rustls::pki_types::{CertificateDer, ServerName};
use ulo_net::rustls::{self, ClientConfig, ClientConnection, RootCertStore, ServerConnection};
use ulo_net::{Tls, TlsError};

const CA: &[u8] = include_bytes!("fixtures/ca.pem");
const CERT: &[u8] = include_bytes!("fixtures/localhost.pem");
const KEY: &[u8] = include_bytes!("fixtures/localhost-key.pem");

fn client(alpn: &[&[u8]]) -> ClientConnection {
    let mut roots = RootCertStore::empty();
    roots.add(CertificateDer::from_pem_slice(CA).expect("the CA parses")).expect("the CA is a trust anchor");
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("the default versions")
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = alpn.iter().map(|protocol| protocol.to_vec()).collect();
    ClientConnection::new(Arc::new(config), ServerName::try_from("localhost").expect("a DNS name")).expect("the client starts")
}

#[test]
fn a_loaded_configuration_completes_a_handshake_without_a_runtime() {
    let config = Tls::from_pem(CERT, KEY).load(&[b"h2", b"http/1.1"]).expect("the pair loads");
    let mut server = ServerConnection::new(config).expect("the server starts");
    let mut client = client(&[b"h2"]);

    let mut wire = Vec::new();
    while client.is_handshaking() || server.is_handshaking() {
        wire.clear();
        client.write_tls(&mut wire).expect("the client writes its records");
        server.read_tls(&mut wire.as_slice()).expect("the server reads them");
        server.process_new_packets().expect("the server accepts the client's records");
        wire.clear();
        server.write_tls(&mut wire).expect("the server writes its records");
        client.read_tls(&mut wire.as_slice()).expect("the client reads them");
        client.process_new_packets().expect("the client accepts the server's certificate");
    }
    assert_eq!(server.alpn_protocol(), Some(b"h2".as_slice()), "the ALPN offered at load settles the protocol");

    client.writer().write_all(b"ping").expect("the client writes");
    wire.clear();
    client.write_tls(&mut wire).expect("the client writes its record");
    server.read_tls(&mut wire.as_slice()).expect("the server reads it");
    server.process_new_packets().expect("the server decrypts it");
    let mut read = [0u8; 4];
    server.reader().read_exact(&mut read).expect("the server reads the plaintext");
    assert_eq!(&read, b"ping");
}

#[test]
fn load_offers_the_protocols_it_is_given_in_order() {
    let config = Tls::from_pem(CERT, KEY).load(&[b"h2", b"http/1.1"]).expect("the pair loads");
    assert_eq!(config.alpn_protocols, vec![b"h2".to_vec(), b"http/1.1".to_vec()]);
}

#[test]
fn a_key_that_is_not_the_certificates_is_refused() {
    let refused = Tls::from_pem(CA, KEY).load(&[]).expect_err("a key not matching the certificate is refused");
    assert!(matches!(refused, TlsError::Rustls(_)), "refused by rustls, got {refused:?}");
}
