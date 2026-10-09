//! The client app's close releases its UDP client socket: `RpcClientModule`'s destroy hook calls
//! the link's `close`, which ends the reply lane of the socket the client bound and releases it.
//! The conformance suite's `client_close` scenario asserts the rest on every link: the waiting call
//! failing `Unavailable` and a later call connecting again. A UDP environment has no connection
//! for it to count, which is what the release stands for here.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Mutex;
use std::time::Duration;

use tokio::net::UdpSocket;
use ulo::{App, Bound, Module, ModuleDef, ModuleIdentity, Signal};
use ulo_rpc::{RpcClient, RpcClientModule};
use ulo_rpc_udp::Udp;

/// Longer than any wait below, so the call is still waiting at the close.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the close has to show at the client's port.
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
async fn closing_the_client_app_releases_its_socket() {
    // A peer that reads the call and never answers it.
    let peer = UdpSocket::bind("127.0.0.1:0").await.expect("a loopback port");
    let addr = peer.local_addr().expect("the peer's address");
    let app = App::builder(ClientRoot { link: Mutex::new(Some(Udp::new(addr))) })
        .runtime(ulo_tokio::Tokio::current())
        .wire()
        .expect("the client app wires")
        .connect()
        .await
        .expect("the client app connects");
    let rpc = (*app.get::<RpcClient>().await.expect("the app holds an `RpcClient`")).clone();
    let _call = tokio::spawn(async move { rpc.request::<String, String>("held", &"x".to_owned()).await });

    let mut datagram = vec![0u8; 65_536];
    let (_, client) = tokio::time::timeout(PATIENCE, peer.recv_from(&mut datagram))
        .await
        .expect("the call's datagram arrives")
        .expect("the peer receives");

    if let Err(error) = app.close(Signal::new("client_close")).await {
        panic!("the client app's close reported a failure: {error}");
    }

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
