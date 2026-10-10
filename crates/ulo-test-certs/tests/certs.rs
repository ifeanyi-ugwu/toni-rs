//! What each certificate is for, shown by a rustls handshake over plain buffers: the self-signed
//! one trusted directly by a client, and a CA's certificates verified on both sides.

use std::sync::Arc;

use rustls::crypto::ring::default_provider;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use rustls::server::WebPkiClientVerifier;
use rustls::{ClientConfig, ClientConnection, ConfigBuilder, ServerConfig, ServerConnection, WantsVerifier};
use ulo_test_certs::{Ca, Certified, localhost};

fn server_builder() -> ConfigBuilder<ServerConfig, WantsVerifier> {
    ServerConfig::builder_with_provider(Arc::new(default_provider())).with_safe_default_protocol_versions().expect("the default versions")
}

fn client_builder() -> ConfigBuilder<ClientConfig, WantsVerifier> {
    ClientConfig::builder_with_provider(Arc::new(default_provider())).with_safe_default_protocol_versions().expect("the default versions")
}

fn chain(cert: &Certified) -> Vec<CertificateDer<'static>> {
    vec![CertificateDer::from_pem_slice(cert.cert_pem().as_bytes()).expect("the certificate's PEM parses")]
}

fn key(cert: &Certified) -> PrivateKeyDer<'static> {
    PrivateKeyDer::from_pem_slice(cert.key_pem().as_bytes()).expect("the key's PEM parses")
}

/// Exchanges records until neither side is handshaking, failing on the first record a side refuses.
fn handshake(client: &mut ClientConnection, server: &mut ServerConnection) {
    let mut wire = Vec::new();
    while client.is_handshaking() || server.is_handshaking() {
        wire.clear();
        client.write_tls(&mut wire).expect("the client writes its records");
        server.read_tls(&mut wire.as_slice()).expect("the server reads them");
        server.process_new_packets().expect("the server accepts the client's records");
        wire.clear();
        server.write_tls(&mut wire).expect("the server writes its records");
        client.read_tls(&mut wire.as_slice()).expect("the client reads them");
        client.process_new_packets().expect("the client accepts the server's records");
    }
}

#[test]
fn a_client_trusting_the_localhost_certificate_completes_a_handshake_with_it() {
    let cert = localhost();
    let server = server_builder().with_no_client_auth().with_single_cert(chain(cert), key(cert)).expect("the pair loads");
    let client = client_builder().with_root_certificates(cert.roots()).with_no_client_auth();
    for name in ["localhost", "127.0.0.1"] {
        let mut server = ServerConnection::new(Arc::new(server.clone())).expect("the server starts");
        let mut client = ClientConnection::new(Arc::new(client.clone()), ServerName::try_from(name).expect("a server name").to_owned())
            .expect("the client starts");
        handshake(&mut client, &mut server);
    }
}

#[test]
fn a_server_verifies_a_client_certificate_its_ca_issued() {
    let ca = Ca::new();
    let served = ca.issue(&["localhost"]);
    let presented = ca.issue(&["client"]);
    let verifier = WebPkiClientVerifier::builder_with_provider(Arc::new(ca.roots()), Arc::new(default_provider()))
        .build()
        .expect("a verifier trusting the CA");
    let server = server_builder().with_client_cert_verifier(verifier).with_single_cert(chain(&served), key(&served)).expect("the pair loads");
    let client = client_builder()
        .with_root_certificates(ca.roots())
        .with_client_auth_cert(chain(&presented), key(&presented))
        .expect("the client's pair loads");
    let mut server = ServerConnection::new(Arc::new(server)).expect("the server starts");
    let mut client = ClientConnection::new(Arc::new(client), ServerName::try_from("localhost").expect("a DNS name")).expect("the client starts");
    handshake(&mut client, &mut server);
    let peer = server.peer_certificates().expect("the server holds the client's certificate");
    assert_eq!(peer.first(), Some(presented.cert_der()), "the certificate the server verified is the client's");
}
