//! `Tls::load` answers a rustls `ServerConfig` that completes a handshake with no runtime at all:
//! the two rustls connections below exchange their records through plain buffers, so a backend on
//! any runtime gets TLS by wrapping the same configuration in its own TLS crate.
//!
//! The certificate is `ulo-test-certs`' self-signed one for `localhost`, made when the tests run.

use std::io::{Read, Write};
use std::sync::Arc;

use ulo_net::rustls::pki_types::ServerName;
use ulo_net::rustls::{self, ClientConfig, ClientConnection, ServerConnection};
use ulo_net::{Tls, TlsError};
use ulo_test_certs::{Certified, localhost};

fn served() -> Tls {
    let cert = localhost();
    Tls::from_pem(cert.cert_pem(), cert.key_pem())
}

fn client(alpn: &[&[u8]]) -> ClientConnection {
    let roots = localhost().roots();
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
    let config = served().load(&[b"h2", b"http/1.1"]).expect("the pair loads");
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
    let config = served().load(&[b"h2", b"http/1.1"]).expect("the pair loads");
    assert_eq!(config.alpn_protocols, vec![b"h2".to_vec(), b"http/1.1".to_vec()]);
}

#[test]
fn a_key_that_is_not_the_certificates_is_refused() {
    let other = Certified::self_signed(&["localhost"]);
    let refused = Tls::from_pem(other.cert_pem(), localhost().key_pem()).load(&[]).expect_err("a key not matching the certificate is refused");
    assert!(matches!(refused, TlsError::Rustls(_)), "refused by rustls, got {refused:?}");
}
