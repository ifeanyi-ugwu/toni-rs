//! The client app's close ends its UDP client side: `RpcClientModule`'s destroy hook calls the
//! link's `close`, which ends the reply lane of the socket the client bound, releases the socket,
//! and fails the call waiting on it.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Mutex;
use std::time::Duration;

use tokio::net::UdpSocket;
use ulo::{App, Bound, Module, ModuleDef, ModuleIdentity, Signal};
use ulo_rpc::{RpcClient, RpcClientModule};
use ulo_rpc_udp::Udp;
use ulo_transport::ErrorKind;

/// Longer than any wait below, so a call that ends early ended for the close.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the close has to show at the waiting call and at the client's port.
const PATIENCE: Duration = Duration::from_secs(5);

struct ClientRoot {
    link: Mutex<Option<Udp>>,
}

impl Module for ClientRoot {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        if let Some(link) = self.link.lock().unwrap().take() {
            m.import(RpcClientModule::for_root(link).timeout(Bound::After(CALL_TIMEOUT)));
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_the_client_app_ends_its_client_side() {
    // A peer that reads the call and never answers it.
    let peer = UdpSocket::bind("127.0.0.1:0").await.expect("a loopback port");
    let addr = peer.local_addr().expect("the peer's address");
    let app = App::builder(ClientRoot { link: Mutex::new(Some(Udp::new(addr))) })
        .timer(ulo_tokio::Timer)
        .wire()
        .expect("the client app wires")
        .connect()
        .await
        .expect("the client app connects");
    let rpc = (*app.get::<RpcClient>().await.expect("the app holds an `RpcClient`")).clone();
    let call = tokio::spawn(async move { rpc.request::<String, String>("held", &"x".to_owned()).await });

    let mut datagram = vec![0u8; 65_536];
    let (_, client) = tokio::time::timeout(PATIENCE, peer.recv_from(&mut datagram))
        .await
        .expect("the call's datagram arrives")
        .expect("the peer receives");

    let _ = app.close(Signal::new("client_close")).await;

    let answered = tokio::time::timeout(PATIENCE, call)
        .await
        .unwrap_or_else(|_| panic!("the waiting call was still waiting {PATIENCE:?} after its app closed"))
        .expect("the call's task does not panic");
    let error = answered.expect_err("no reply was sent, so the call fails");
    assert_eq!(error.kind(), ErrorKind::Unavailable, "the waiting call fails `Unavailable` at the close, got: {error:?}");

    // The client's socket is released: the address it bound, its port on every interface, binds
    // again.
    let bound = SocketAddr::from((Ipv4Addr::UNSPECIFIED, client.port()));
    let released = async {
        loop {
            match UdpSocket::bind(bound).await {
                Ok(_) => return,
                Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
            }
        }
    };
    tokio::time::timeout(PATIENCE, released)
        .await
        .unwrap_or_else(|_| panic!("the client's socket on {bound} was still bound {PATIENCE:?} after its app closed"));
}
